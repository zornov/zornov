// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Blend modes and compositing, after the W3C Compositing and Blending
//! Level 1 specification.
//!
//! The separable and non-separable blend functions `B(Cb, Cs)` take
//! un-premultiplied colours. Compositing then uses the premultiplied form
//! of source-over with blending:
//!
//! ```text
//! co = cs × (1 − αb) + cb × (1 − αs) + αs × αb × B(Cb, Cs)
//! αo = αs + αb × (1 − αs)
//! ```
//!
//! `Add` is not a W3C blend mode; it is the "linear dodge" `B = Cb + Cs`
//! without a clamp, so HDR highlights keep adding up.
//!
//! The CPU functions here are the reference the WGSL (`composite.wgsl`)
//! mirrors line for line.

use crate::frame::{Px, unpremultiply};

/// Blend modes, in the order the shader expects (`as u32`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u32)]
pub enum BlendMode {
    #[default]
    Normal = 0,
    Add = 1,
    Multiply = 2,
    Screen = 3,
    Overlay = 4,
    SoftLight = 5,
    HardLight = 6,
    ColourDodge = 7,
    ColourBurn = 8,
    Darken = 9,
    Lighten = 10,
    Difference = 11,
    Exclusion = 12,
    Hue = 13,
    Saturation = 14,
    Colour = 15,
    Luminosity = 16,
    /// Track mattes: the canvas below is kept where this layer is opaque
    /// (stencil) or transparent (silhouette), by alpha or by brightness.
    /// The layer itself is not drawn.
    StencilAlpha = 17,
    SilhouetteAlpha = 18,
    StencilLuma = 19,
    SilhouetteLuma = 20,
}

impl BlendMode {
    /// Every mode, in shader order.
    pub const ALL: [BlendMode; 21] = [
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::ColourDodge,
        BlendMode::ColourBurn,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Colour,
        BlendMode::Luminosity,
        BlendMode::StencilAlpha,
        BlendMode::SilhouetteAlpha,
        BlendMode::StencilLuma,
        BlendMode::SilhouetteLuma,
    ];

    /// Stable identifier for project files and golden file names.
    pub fn id(self) -> &'static str {
        match self {
            BlendMode::Normal => "normal",
            BlendMode::Add => "add",
            BlendMode::Multiply => "multiply",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::SoftLight => "soft_light",
            BlendMode::HardLight => "hard_light",
            BlendMode::ColourDodge => "colour_dodge",
            BlendMode::ColourBurn => "colour_burn",
            BlendMode::Darken => "darken",
            BlendMode::Lighten => "lighten",
            BlendMode::Difference => "difference",
            BlendMode::Exclusion => "exclusion",
            BlendMode::Hue => "hue",
            BlendMode::Saturation => "saturation",
            BlendMode::Colour => "colour",
            BlendMode::Luminosity => "luminosity",
            BlendMode::StencilAlpha => "stencil_alpha",
            BlendMode::SilhouetteAlpha => "silhouette_alpha",
            BlendMode::StencilLuma => "stencil_luma",
            BlendMode::SilhouetteLuma => "silhouette_luma",
        }
    }

    pub fn from_id(id: &str) -> Option<BlendMode> {
        BlendMode::ALL.iter().copied().find(|m| m.id() == id)
    }

    /// A track-matte mode: it changes the canvas everywhere, inside the
    /// layer and out.
    pub fn is_matte(self) -> bool {
        matches!(self, BlendMode::StencilAlpha | BlendMode::SilhouetteAlpha | BlendMode::StencilLuma | BlendMode::SilhouetteLuma)
    }

    fn separable(self) -> bool {
        !matches!(self, BlendMode::Hue | BlendMode::Saturation | BlendMode::Colour | BlendMode::Luminosity)
    }
}

// ---- separable modes (per channel) ----------------------------------

#[inline]
fn screen(cb: f32, cs: f32) -> f32 {
    cb + cs - cb * cs
}

#[inline]
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 { cb * 2.0 * cs } else { screen(cb, 2.0 * cs - 1.0) }
}

#[inline]
fn soft_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
    } else {
        let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
        cb + (2.0 * cs - 1.0) * (d - cb)
    }
}

#[inline]
fn colour_dodge(cb: f32, cs: f32) -> f32 {
    if cb <= 0.0 {
        0.0
    } else if cs >= 1.0 {
        1.0
    } else {
        (cb / (1.0 - cs)).min(1.0)
    }
}

#[inline]
fn colour_burn(cb: f32, cs: f32) -> f32 {
    if cb >= 1.0 {
        1.0
    } else if cs <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - cb) / cs).min(1.0)
    }
}

/// One channel of a separable blend.
#[inline]
pub fn blend_channel(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    match mode {
        BlendMode::Normal => cs,
        BlendMode::Add => cb + cs,
        BlendMode::Multiply => cb * cs,
        BlendMode::Screen => screen(cb, cs),
        BlendMode::Overlay => hard_light(cs, cb),
        BlendMode::SoftLight => soft_light(cb, cs),
        BlendMode::HardLight => hard_light(cb, cs),
        BlendMode::ColourDodge => colour_dodge(cb, cs),
        BlendMode::ColourBurn => colour_burn(cb, cs),
        BlendMode::Darken => cb.min(cs),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::Difference => (cb - cs).abs(),
        BlendMode::Exclusion => cb + cs - 2.0 * cb * cs,
        _ => cs,
    }
}

// ---- non-separable modes ----------------------------------------------

#[inline]
fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

#[inline]
fn clip_colour(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        for v in out.iter_mut() {
            *v = l + (*v - l) * l / (l - n).max(1e-9);
        }
    }
    if x > 1.0 {
        for v in out.iter_mut() {
            *v = l + (*v - l) * (1.0 - l) / (x - l).max(1e-9);
        }
    }
    out
}

#[inline]
fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_colour([c[0] + d, c[1] + d, c[2] + d])
}

#[inline]
fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

#[inline]
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    // Indices of max, mid, min.
    let (mut imax, mut imid, mut imin) = (0usize, 1usize, 2usize);
    if c[imax] < c[imid] {
        std::mem::swap(&mut imax, &mut imid);
    }
    if c[imid] < c[imin] {
        std::mem::swap(&mut imid, &mut imin);
    }
    if c[imax] < c[imid] {
        std::mem::swap(&mut imax, &mut imid);
    }
    let mut out = [0.0; 3];
    let range = c[imax] - c[imin];
    if range > 0.0 {
        out[imid] = (c[imid] - c[imin]) * s / range;
        out[imax] = s;
    }
    out
}

/// A non-separable blend of un-premultiplied colours.
#[inline]
pub fn blend_rgb(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        BlendMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        BlendMode::Colour => set_lum(cs, lum(cb)),
        BlendMode::Luminosity => set_lum(cb, lum(cs)),
        m => [blend_channel(m, cb[0], cs[0]), blend_channel(m, cb[1], cs[1]), blend_channel(m, cb[2], cs[2])],
    }
}

/// Composites premultiplied source `s` over premultiplied backdrop `b`.
#[inline]
pub fn composite(mode: BlendMode, b: Px, s: Px) -> Px {
    if mode.is_matte() {
        return matte(mode, b, s);
    }
    let (ab, as_) = (b[3], s[3]);
    if as_ <= 0.0 {
        return b;
    }
    if mode == BlendMode::Normal {
        return [s[0] + b[0] * (1.0 - as_), s[1] + b[1] * (1.0 - as_), s[2] + b[2] * (1.0 - as_), as_ + ab * (1.0 - as_)];
    }
    let ub = unpremultiply(b);
    let us = unpremultiply(s);
    let blended = if mode.separable() {
        [blend_channel(mode, ub[0], us[0]), blend_channel(mode, ub[1], us[1]), blend_channel(mode, ub[2], us[2])]
    } else {
        // The W3C non-separable formulas are defined on 0..1 colours; HDR
        // values are clamped for the luminance / saturation bookkeeping.
        let cb = [ub[0].clamp(0.0, 1.0), ub[1].clamp(0.0, 1.0), ub[2].clamp(0.0, 1.0)];
        let cs = [us[0].clamp(0.0, 1.0), us[1].clamp(0.0, 1.0), us[2].clamp(0.0, 1.0)];
        blend_rgb(mode, cb, cs)
    };
    let k = as_ * ab;
    [
        s[0] * (1.0 - ab) + b[0] * (1.0 - as_) + k * blended[0],
        s[1] * (1.0 - ab) + b[1] * (1.0 - as_) + k * blended[1],
        s[2] * (1.0 - ab) + b[2] * (1.0 - as_) + k * blended[2],
        as_ + ab * (1.0 - as_),
    ]
}

/// A track matte: the canvas `b` kept by how much of the (premultiplied)
/// matte `s` covers it.
#[inline]
pub fn matte(mode: BlendMode, b: Px, s: Px) -> Px {
    let a = s[3].clamp(0.0, 1.0);
    let luma = || {
        let u = unpremultiply(s);
        (0.2126 * u[0] + 0.7152 * u[1] + 0.0722 * u[2]).clamp(0.0, 1.0) * a
    };
    let k = match mode {
        BlendMode::StencilAlpha => a,
        BlendMode::SilhouetteAlpha => 1.0 - a,
        BlendMode::StencilLuma => luma(),
        BlendMode::SilhouetteLuma => 1.0 - luma(),
        _ => 1.0,
    };
    [b[0] * k, b[1] * k, b[2] * k, b[3] * k]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_backdrop_identities() {
        let b = [0.3, 0.6, 0.9, 1.0];
        // Normal over opaque = source.
        assert_eq!(composite(BlendMode::Normal, b, [0.1, 0.2, 0.3, 1.0]), [0.1, 0.2, 0.3, 1.0]);
        // Multiply with white = backdrop; screen with black = backdrop.
        let m = composite(BlendMode::Multiply, b, [1.0, 1.0, 1.0, 1.0]);
        let s = composite(BlendMode::Screen, b, [0.0, 0.0, 0.0, 1.0]);
        for i in 0..3 {
            assert!((m[i] - b[i]).abs() < 1e-6);
            assert!((s[i] - b[i]).abs() < 1e-6);
        }
        // Difference with itself is black.
        let d = composite(BlendMode::Difference, b, b);
        assert!(d[..3].iter().all(|c| c.abs() < 1e-6));
    }

    #[test]
    fn transparent_source_leaves_backdrop() {
        let b = [0.3, 0.6, 0.9, 0.5];
        for m in BlendMode::ALL.into_iter().filter(|m| !m.is_matte()) {
            assert_eq!(composite(m, b, [0.0; 4]), b);
        }
    }

    #[test]
    fn transparent_backdrop_shows_source() {
        let s = [0.2, 0.4, 0.1, 0.5];
        for m in BlendMode::ALL.into_iter().filter(|m| !m.is_matte()) {
            let o = composite(m, [0.0; 4], s);
            for i in 0..4 {
                assert!((o[i] - s[i]).abs() < 1e-6, "{m:?}");
            }
        }
    }

    #[test]
    fn mattes_keep_the_canvas_by_cover_or_brightness() {
        let b = [0.4, 0.2, 0.6, 1.0];
        // Fully covered, half covered, uncovered.
        assert_eq!(composite(BlendMode::StencilAlpha, b, [1.0, 1.0, 1.0, 1.0]), b);
        assert_eq!(composite(BlendMode::StencilAlpha, b, [0.0; 4]), [0.0; 4]);
        assert_eq!(composite(BlendMode::SilhouetteAlpha, b, [0.0; 4]), b);
        let half = composite(BlendMode::StencilAlpha, b, [0.25, 0.25, 0.25, 0.5]);
        assert!((half[3] - 0.5).abs() < 1e-6);
        // By brightness: a black matte hides, a white one shows.
        assert_eq!(composite(BlendMode::StencilLuma, b, [0.0, 0.0, 0.0, 1.0])[3], 0.0);
        assert!((composite(BlendMode::StencilLuma, b, [1.0, 1.0, 1.0, 1.0])[3] - 1.0).abs() < 1e-6);
        assert!((composite(BlendMode::SilhouetteLuma, b, [0.0, 0.0, 0.0, 1.0])[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn overlay_is_hard_light_swapped() {
        for &(cb, cs) in &[(0.2, 0.7), (0.8, 0.3), (0.5, 0.5)] {
            assert!((blend_channel(BlendMode::Overlay, cb, cs) - blend_channel(BlendMode::HardLight, cs, cb)).abs() < 1e-6);
        }
    }

    #[test]
    fn soft_light_reference_points() {
        // From the W3C formula: Cs = 0.5 leaves Cb unchanged.
        for cb in [0.0, 0.1, 0.25, 0.5, 0.9, 1.0] {
            assert!((soft_light(cb, 0.5) - cb).abs() < 1e-6);
        }
        // Lighter source brightens, darker darkens.
        assert!(soft_light(0.5, 0.8) > 0.5);
        assert!(soft_light(0.5, 0.2) < 0.5);
    }

    #[test]
    fn non_separable_stay_in_range() {
        let cb = [0.9, 0.2, 0.1];
        let cs = [0.1, 0.3, 0.95];
        for m in [BlendMode::Hue, BlendMode::Saturation, BlendMode::Colour, BlendMode::Luminosity] {
            let o = blend_rgb(m, cb, cs);
            assert!(o.iter().all(|c| (-1e-6..=1.0 + 1e-6).contains(c)), "{m:?} {o:?}");
        }
        // Luminosity keeps the source's luminance, colour keeps the backdrop's.
        assert!((lum(blend_rgb(BlendMode::Luminosity, cb, cs)) - lum(cs)).abs() < 1e-4);
        assert!((lum(blend_rgb(BlendMode::Colour, cb, cs)) - lum(cb)).abs() < 1e-4);
    }
}
