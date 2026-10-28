// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Materials for what a figure wears: cloth with a weave you can see up
//! close (cotton, linen, wool, tweed, denim, knit, silk, satin, velvet,
//! felt, canvas), leather and suede, rubber and plastic, metals (steel,
//! iron, brass, gold, silver, bronze, copper). Each is a physically based
//! material and, for the woven, knitted and grained ones, a small tileable
//! texture carrying the colour, so a coat reads as wool and not as a flat
//! grey shell.
//!
//! A pattern (`stripes`, `pinstripe`, `check`, `plaid`, `dots`) goes over
//! any cloth in a second colour, at the size it has on real clothes (a
//! gingham check about 1.5% of the wearer's height, a tartan repeat about
//! 9%), with the weave repeating inside it at its own finer size, so a
//! pattern never shrinks to the weave's scale and breaks up into blotches.
//! The weave and its mottling are kept faint: cloth reads as cloth, not as
//! camouflage.

use crate::{Material, Texture, srgb_to_linear};

/// Every material name [`material`] knows.
pub const NAMES: &str = "cotton, linen, wool, tweed, flannel, denim, knit, silk, satin, velvet, felt, canvas, nylon, leather, suede, rubber, plastic, steel, iron, brass, gold, silver, bronze, copper, fur";

/// A woven pattern over the cloth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    None,
    Stripes,
    Pinstripe,
    Check,
    Plaid,
    Dots,
}

impl Pattern {
    /// One repeat of the pattern as a fraction of the wearer's height.
    fn repeat(self) -> f32 {
        match self {
            Pattern::None => 0.0,
            Pattern::Stripes => 0.035,
            Pattern::Pinstripe => 0.022,
            Pattern::Check => 0.03,
            Pattern::Plaid => 0.09,
            Pattern::Dots => 0.03,
        }
    }

    pub fn parse(s: &str) -> Option<Pattern> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "plain" | "solid" => Pattern::None,
            "stripes" | "stripe" | "striped" => Pattern::Stripes,
            "pinstripe" | "pinstripes" | "pin" => Pattern::Pinstripe,
            "check" | "checks" | "checked" | "gingham" | "checkered" => Pattern::Check,
            "plaid" | "tartan" => Pattern::Plaid,
            "dots" | "polka" | "spots" => Pattern::Dots,
            _ => return None,
        })
    }
}

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA6B) ^ seed.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0xFFFF) as f32 / 65535.0
}

/// Value noise that tiles every `period` cells.
fn noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |i: i32, j: i32| hash(i.rem_euclid(period), j.rem_euclid(period), seed);
    let (i, j) = (x0 as i32, y0 as i32);
    let a = at(i, j) * (1.0 - sx) + at(i + 1, j) * sx;
    let b = at(i, j + 1) * (1.0 - sx) + at(i + 1, j + 1) * sx;
    a * (1.0 - sy) + b * sy
}

/// What a named material is: roughness, metal, how much light it takes in,
/// and the grain of its texture.
struct Spec {
    roughness: f32,
    metallic: f32,
    subsurface: f32,
    /// Brightness change across the grain (0 none).
    grain: f32,
    /// The weave: `Weave` pattern of the texture.
    weave: Weave,
    /// Texture repeat length in model px at a figure 400 px tall.
    tile: f32,
}

#[derive(Clone, Copy)]
enum Weave {
    None,
    Plain,
    Twill,
    Herringbone,
    Knit,
    Cells,
    Fuzz,
}

fn spec(name: &str) -> Option<Spec> {
    let s = |roughness, metallic, subsurface, grain, weave, tile| Some(Spec { roughness, metallic, subsurface, grain, weave, tile });
    match name.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "cotton" | "tee" | "jersey" | "fabric" | "cloth" | "linen_cotton" => s(0.88, 0.0, 0.04, 0.05, Weave::Plain, 5.0),
        "linen" => s(0.9, 0.0, 0.04, 0.09, Weave::Plain, 7.0),
        "wool" | "woollen" | "woolen" | "gabardine" | "twill" => s(0.93, 0.0, 0.05, 0.1, Weave::Twill, 6.0),
        "tweed" | "herringbone" => s(0.95, 0.0, 0.05, 0.16, Weave::Herringbone, 9.0),
        "flannel" | "fleece" => s(0.97, 0.0, 0.06, 0.1, Weave::Fuzz, 6.0),
        "denim" | "jeans" => s(0.86, 0.0, 0.03, 0.16, Weave::Twill, 5.0),
        "knit" | "knitted" | "sweater" | "wool_knit" => s(0.96, 0.0, 0.05, 0.2, Weave::Knit, 10.0),
        "silk" => s(0.34, 0.0, 0.05, 0.02, Weave::None, 6.0),
        "satin" => s(0.26, 0.0, 0.04, 0.02, Weave::None, 6.0),
        "velvet" => s(0.97, 0.0, 0.1, 0.07, Weave::Fuzz, 5.0),
        "felt" => s(0.98, 0.0, 0.06, 0.1, Weave::Fuzz, 6.0),
        "canvas" | "duck" => s(0.92, 0.0, 0.02, 0.12, Weave::Plain, 8.0),
        "nylon" | "polyester" | "shell" => s(0.5, 0.0, 0.02, 0.03, Weave::Plain, 4.0),
        "leather" | "hide" => s(0.5, 0.0, 0.02, 0.14, Weave::Cells, 12.0),
        "suede" => s(0.95, 0.0, 0.03, 0.12, Weave::Fuzz, 8.0),
        "rubber" => s(0.78, 0.0, 0.0, 0.03, Weave::None, 8.0),
        "plastic" => s(0.38, 0.0, 0.0, 0.0, Weave::None, 8.0),
        "steel" | "iron" | "metal" | "armour" | "armor" | "plate" => s(0.38, 1.0, 0.0, 0.03, Weave::None, 10.0),
        "brass" | "bronze" | "copper" | "gold" | "silver" | "chrome" => s(0.3, 1.0, 0.0, 0.0, Weave::None, 10.0),
        "fur" | "pelt" => s(0.96, 0.0, 0.08, 0.2, Weave::Fuzz, 5.0),
        _ => None,
    }
}

/// The colour a named metal has when none is asked for (sRGB).
pub fn metal_colour(name: &str) -> Option<[u8; 3]> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "steel" | "metal" | "armour" | "armor" | "plate" => [178, 182, 190],
        "iron" => [96, 98, 104],
        "brass" => [196, 162, 82],
        "bronze" => [170, 120, 70],
        "copper" => [200, 120, 90],
        "gold" => [232, 190, 80],
        "silver" | "chrome" => [214, 216, 220],
        _ => return None,
    })
}

/// A material by name in a colour (sRGB), with a texture at `size` px
/// across; `second` and `pattern` lay a woven pattern over the cloth. The
/// texture repeats every `Spec::tile` px of a figure `scale` px tall, so ask
/// the UVs for `extent / tile` repeats (see [`tile_px`]).
pub fn material(name: &str, colour: Option<[u8; 3]>, second: Option<[u8; 3]>, pattern: Pattern, size: usize) -> Option<Material> {
    let sp = spec(name)?;
    let colour = colour.or_else(|| metal_colour(name)).unwrap_or([128, 128, 128]);
    let lin = [srgb_to_linear(colour[0]), srgb_to_linear(colour[1]), srgb_to_linear(colour[2]), 1.0];
    let mut m = Material { name: name.to_string(), base_colour: lin, metallic: sp.metallic, roughness: sp.roughness, subsurface: sp.subsurface, ..Material::default() };
    if matches!(sp.weave, Weave::None) && pattern == Pattern::None || size < 8 {
        return Some(m);
    }
    let second = second.unwrap_or([(colour[0] as f32 * 0.55) as u8, (colour[1] as f32 * 0.55) as u8, (colour[2] as f32 * 0.55) as u8]);
    let weaves = weaves_per_repeat(&sp, pattern);
    // Whole weave tiles of a size the noise tiles in (a multiple of 8).
    let wn = ((size * if weaves > 1 { 2 } else { 1 }).clamp(16, 512) / weaves / 8 * 8).max(8);
    let n = wn * weaves;
    m.texture = Some(texture(&sp, colour, second, pattern, n, weaves));
    m.base_colour = [1.0, 1.0, 1.0, 1.0];
    Some(m)
}

/// Texture repeat length (model px) of a material for a figure `figure_px`
/// tall (so cloth weaves stay the same size on a child and a giant).
pub fn tile_px(name: &str, figure_px: f32) -> f32 {
    spec(name).map_or(8.0, |s| s.tile) * figure_px / 400.0 * 4.0
}

/// How many weave tiles one repeat of the pattern holds (1 without one).
fn weaves_per_repeat(sp: &Spec, pattern: Pattern) -> usize {
    if pattern == Pattern::None {
        return 1;
    }
    // The weave's tile is `tile / 100` of the figure's height.
    ((pattern.repeat() * 100.0 / sp.tile).round() as usize).clamp(1, 8)
}

/// Texture repeat length (model px) of a material with a pattern on a
/// figure `figure_px` tall: one repeat of the pattern (a whole number of
/// weave tiles), or the weave's own tile when plain.
pub fn repeat_px(name: &str, pattern: Pattern, figure_px: f32) -> f32 {
    let weaves = spec(name).map_or(1, |s| weaves_per_repeat(&s, pattern));
    tile_px(name, figure_px) * weaves as f32
}

fn texture(sp: &Spec, base: [u8; 3], second: [u8; 3], pattern: Pattern, n: usize, weaves: usize) -> Texture {
    let mut rgba = vec![255u8; n * n * 4];
    // The weave tiles `weaves` times across the texture; the pattern once.
    let wn = n / weaves.max(1);
    let p = wn as i32;
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32, y as f32);
            let (fx, fy) = ((x % wn) as f32, (y % wn) as f32);
            let n_w = wn;
            // The weave: a brightness factor around 1.
            let w = match sp.weave {
                Weave::None => 1.0,
                Weave::Plain => {
                    let c = 8.0;
                    let a = ((fx * c / n_w as f32 * std::f32::consts::TAU).sin() * (fy * c / n_w as f32 * std::f32::consts::TAU).sin()).signum();
                    1.0 + 0.5 * sp.grain * a * 0.6
                }
                Weave::Twill => {
                    let c = 16.0;
                    let d = ((fx + fy) * c / n_w as f32 * std::f32::consts::TAU / 2.0).sin();
                    1.0 + sp.grain * 0.5 * d
                }
                Weave::Herringbone => {
                    let c = 16.0;
                    let flip = if ((fx * 4.0 / n_w as f32) as i32) % 2 == 0 { 1.0 } else { -1.0 };
                    let d = ((fx * flip + fy) * c / n_w as f32 * std::f32::consts::TAU / 2.0).sin();
                    1.0 + sp.grain * 0.5 * d
                }
                Weave::Knit => {
                    // Columns of V stitches.
                    let (cx, cy) = (n_w as f32 / 8.0, n_w as f32 / 10.0);
                    let u = (fx % cx) / cx - 0.5;
                    let v = (fy % cy) / cy;
                    let d = (u.abs() * 1.6 - v).abs();
                    1.0 + sp.grain * (0.5 - d.min(1.0)).clamp(-0.5, 0.5) * 2.0
                }
                Weave::Cells => {
                    // Grain of cells: the distance to the nearest of a jittered grid of points.
                    let g = 14;
                    let (gx, gy) = (fx / n_w as f32 * g as f32, fy / n_w as f32 * g as f32);
                    let (ix, iy) = (gx.floor() as i32, gy.floor() as i32);
                    let mut best = 9.0f32;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let (cx, cy) = (ix + dx, iy + dy);
                            let (jx, jy) = (hash(cx.rem_euclid(g), cy.rem_euclid(g), 3), hash(cx.rem_euclid(g), cy.rem_euclid(g), 7));
                            let d = ((cx as f32 + jx) - gx).powi(2) + ((cy as f32 + jy) - gy).powi(2);
                            best = best.min(d);
                        }
                    }
                    1.0 - sp.grain * (best.sqrt() - 0.4)
                }
                Weave::Fuzz => 1.0 + sp.grain * (noise(fx / 2.0, fy / 2.0, p / 2, 1) - 0.5) * 2.0,
            };
            // Faint, fine mottling over everything (never patches).
            let mott = 1.0 + 0.2 * sp.grain * (noise(fx / 4.0, fy / 4.0, p / 4, 5) - 0.5);
            let w = 1.0 + (w - 1.0) * 0.6;
            // The pattern picks between the two colours, over the whole texture.
            let (fx, fy) = (px, py);
            let pick = match pattern {
                Pattern::None => 0.0,
                // One repeat across the texture: a stripe and its gap, one
                // pinstripe, two by two checks, one tartan sett, one dot.
                Pattern::Stripes => if ((fx * 2.0 / n as f32) as i32) % 2 == 0 { 0.0 } else { 1.0 },
                Pattern::Pinstripe => if (fx / n as f32).fract() < 0.08 { 1.0 } else { 0.0 },
                Pattern::Check => {
                    let (a, b) = (((fx * 2.0 / n as f32) as i32) % 2, ((fy * 2.0 / n as f32) as i32) % 2);
                    if a == b { 0.0 } else { 1.0 }
                }
                Pattern::Plaid => {
                    let a: f32 = if (fx / n as f32).fract() < 0.28 { 0.6 } else { 0.0 };
                    let b: f32 = if (fy / n as f32).fract() < 0.28 { 0.6 } else { 0.0 };
                    (a + b).min(1.0f32).max(if (fx * 4.0 / n as f32).fract() < 0.06 || (fy * 4.0 / n as f32).fract() < 0.06 { 0.85 } else { 0.0 })
                }
                Pattern::Dots => {
                    let (cx, cy) = ((fx / n as f32).fract() - 0.5, (fy / n as f32).fract() - 0.5);
                    if cx * cx + cy * cy < 0.04 { 1.0 } else { 0.0 }
                }
            };
            let i = (y * n + x) * 4;
            for c in 0..3 {
                let col = base[c] as f32 * (1.0 - pick) + second[c] as f32 * pick;
                rgba[i + c] = (col * w * mott).clamp(0.0, 255.0) as u8;
            }
        }
    }
    Texture { width: n as u32, height: n as u32, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_material_makes_and_the_woven_ones_have_a_texture() {
        for n in NAMES.split(", ") {
            let m = material(n, Some([120, 90, 70]), None, Pattern::None, 64).unwrap_or_else(|| panic!("{n}"));
            assert!((m.roughness - 0.0).abs() <= 1.0);
        }
        let wool = material("wool", Some([120, 90, 70]), None, Pattern::None, 64).unwrap();
        assert!(wool.texture.is_some() && wool.base_colour[0] == 1.0);
        let plaid = material("cotton", Some([180, 40, 30]), Some([20, 20, 20]), Pattern::Plaid, 64).unwrap();
        let t = plaid.texture.unwrap();
        let reds: Vec<u8> = t.rgba.chunks(4).map(|p| p[0]).collect();
        let (lo, hi) = (reds.iter().min().unwrap(), reds.iter().max().unwrap());
        assert!(*hi as i32 - *lo as i32 > 80, "a pattern shows: {lo}..{hi}");
        assert!(material("steel", None, None, Pattern::None, 64).unwrap().metallic > 0.9);
        assert!(material("unobtainium", None, None, Pattern::None, 64).is_none());
    }
}
