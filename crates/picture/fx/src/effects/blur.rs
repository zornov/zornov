// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Blur & focus: gaussian, box, directional, spin, zoom, lens (bokeh),
//! tilt-shift and background blur fill.

use super::{apply_mix, Category, EffectContext, EffectDef, MIX};
use crate::image::{luma, over, scale_px, Edge, Image, Px};
use crate::math::{smoothstep, TAU};
use crate::ops::{box_blur, gaussian_blur, pct_h};
use crate::params::{ParamDef, ParamSet, Unit};

const DIRECTIONS: &[&str] = &["Both", "Horizontal", "Vertical"];

pub static EFFECTS: &[EffectDef] = &[
    EffectDef::new(
        "gaussian_blur",
        "Gaussian blur",
        Category::Blur,
        "Soft, even blur.",
        &[
            ParamDef::float("radius", "Radius", 0.0, 20.0, 1.0).unit(Unit::PercentHeight),
            ParamDef::choice("direction", "Direction", DIRECTIONS, 0),
        ],
        gaussian,
    )
    .identity("radius", 0.0),
    EffectDef::new(
        "box_blur",
        "Box blur",
        Category::Blur,
        "Flat-weighted blur with a slightly harder look.",
        &[
            ParamDef::float("radius", "Radius", 0.0, 20.0, 1.0).unit(Unit::PercentHeight),
            ParamDef::choice("direction", "Direction", DIRECTIONS, 0),
        ],
        boxed,
    )
    .identity("radius", 0.0),
    EffectDef::new(
        "directional_blur",
        "Directional blur",
        Category::Blur,
        "Motion blur along a line.",
        &[
            ParamDef::float("length", "Length", 0.0, 30.0, 4.0).unit(Unit::PercentHeight),
            ParamDef::float("angle", "Angle", -180.0, 180.0, 0.0).unit(Unit::Degrees),
        ],
        directional,
    )
    .identity("length", 0.0)
    .wgsl(crate::shaders::DIRECTIONAL_BLUR),
    EffectDef::new(
        "spin_blur",
        "Spin blur",
        Category::Blur,
        "Rotational blur around a centre point.",
        &[
            ParamDef::float("angle", "Angle", 0.0, 90.0, 8.0).unit(Unit::Degrees),
            ParamDef::point("center", "Centre", [0.5, 0.5]),
        ],
        spin,
    )
    .identity("angle", 0.0)
    .wgsl(crate::shaders::SPIN_BLUR),
    EffectDef::new(
        "zoom_blur",
        "Zoom blur",
        Category::Blur,
        "Streaks rushing out from a centre point.",
        &[
            ParamDef::float("amount", "Amount", 0.0, 1.0, 0.15).unit(Unit::Fraction),
            ParamDef::point("center", "Centre", [0.5, 0.5]),
        ],
        zoom,
    )
    .identity("amount", 0.0)
    .wgsl(crate::shaders::ZOOM_BLUR),
    EffectDef::new(
        "lens_blur",
        "Lens blur",
        Category::Blur,
        "Camera-like defocus with shaped bokeh highlights.",
        &[
            ParamDef::float("radius", "Radius", 0.0, 10.0, 1.5).unit(Unit::PercentHeight),
            ParamDef::choice("shape", "Bokeh shape", &["Disc", "Hexagon", "Octagon", "Star", "Oval"], 1),
            ParamDef::float("rotation", "Shape rotation", -180.0, 180.0, 15.0).unit(Unit::Degrees),
            ParamDef::float("boost", "Highlight boost", 0.0, 8.0, 2.0),
            ParamDef::float("threshold", "Highlight threshold", 0.0, 2.0, 0.7),
        ],
        lens,
    )
    .identity("radius", 0.0),
    EffectDef::new(
        "tilt_shift",
        "Tilt-shift",
        Category::Blur,
        "Sharp band with blur either side, for a miniature look.",
        &[
            ParamDef::float("focus", "Focus position", 0.0, 1.0, 0.55).unit(Unit::Fraction),
            ParamDef::float("width", "Focus width", 0.0, 1.0, 0.15).unit(Unit::Fraction),
            ParamDef::float("falloff", "Falloff", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::float("angle", "Angle", -90.0, 90.0, 0.0).unit(Unit::Degrees),
            ParamDef::float("radius", "Blur", 0.0, 10.0, 1.5).unit(Unit::PercentHeight),
            ParamDef::float("saturation", "Saturation boost", 0.0, 1.0, 0.15).unit(Unit::Fraction),
            MIX,
        ],
        tilt_shift,
    )
    .identity("mix", 0.0),
    EffectDef::new(
        "background_fill",
        "Background blur fill",
        Category::Blur,
        "Fills empty areas around a smaller picture with a blurred, enlarged copy of it.",
        &[
            ParamDef::float("blur", "Blur", 0.0, 20.0, 4.0).unit(Unit::PercentHeight),
            ParamDef::float("dim", "Darken", 0.0, 1.0, 0.2).unit(Unit::Fraction),
            ParamDef::float("zoom", "Extra zoom", 1.0, 2.0, 1.1).unit(Unit::Times),
            MIX,
        ],
        background_fill,
    )
    .identity("mix", 0.0),
];

fn axis_sigmas(img: &Image, p: &ParamSet, scale: f32) -> (f32, f32) {
    let r = pct_h(img, p.f("radius")) * scale;
    match p.choice("direction") {
        1 => (r, 0.0),
        2 => (0.0, r),
        _ => (r, r),
    }
}

fn gaussian(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    // A radius reads best as roughly two standard deviations.
    let (sx, sy) = axis_sigmas(img, p, 0.5);
    gaussian_blur(img, sx, sy, Edge::Clamp)
}

fn boxed(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let (rx, ry) = axis_sigmas(img, p, 1.0);
    box_blur(img, rx, ry, Edge::Clamp)
}

/// Averages `taps` samples along `f(t)` for t evenly spread over -0.5..0.5.
fn line_average(img: &Image, taps: usize, edge: Edge, mut f: impl FnMut(f32) -> (f32, f32)) -> Px {
    let mut acc = [0.0f32; 4];
    for i in 0..taps {
        let t = (i as f32 + 0.5) / taps as f32 - 0.5;
        let (x, y) = f(t);
        let s = img.sample(x, y, edge);
        for c in 0..4 {
            acc[c] += s[c];
        }
    }
    scale_px(acc, 1.0 / taps as f32)
}

/// Tap count for a streak of `len` pixels: about one per pixel, capped so
/// the cost stays bounded (bilinear filtering hides the gaps).
fn taps_for(len: f32) -> usize {
    (len.abs().ceil() as usize).clamp(1, 64)
}

fn directional(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let len = pct_h(img, p.f("length"));
    if len < 0.5 {
        return img.clone();
    }
    let (s, c) = p.f("angle").to_radians().sin_cos();
    let (dx, dy) = (c * len, s * len);
    let taps = taps_for(len);
    Image::from_fn(img.width, img.height, |x, y| {
        let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
        line_average(img, taps, Edge::Clamp, |t| (cx + dx * t, cy + dy * t))
    })
}

fn center_px(img: &Image, p: &ParamSet) -> (f32, f32) {
    let c = p.point("center");
    (c[0] * img.width as f32, c[1] * img.height as f32)
}

fn spin(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let angle = p.f("angle").to_radians();
    if angle <= 0.0 {
        return img.clone();
    }
    let (cx, cy) = center_px(img, p);
    Image::from_fn(img.width, img.height, |x, y| {
        let (vx, vy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let r = (vx * vx + vy * vy).sqrt();
        let taps = taps_for(r * angle);
        line_average(img, taps, Edge::Clamp, |t| {
            let (s, c) = (angle * t).sin_cos();
            (cx + vx * c - vy * s, cy + vx * s + vy * c)
        })
    })
}

fn zoom(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = p.f("amount");
    if amount <= 0.0 {
        return img.clone();
    }
    let (cx, cy) = center_px(img, p);
    Image::from_fn(img.width, img.height, |x, y| {
        let (vx, vy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let r = (vx * vx + vy * vy).sqrt();
        let taps = taps_for(r * amount);
        // Streaks run from the pixel inwards, so the centre stays sharp.
        line_average(img, taps, Edge::Clamp, |t| {
            let k = 1.0 - amount * (t + 0.5);
            (cx + vx * k, cy + vy * k)
        })
    })
}

/// Distance from the centre to the edge of the bokeh shape at angle `a`,
/// for a shape whose outer radius is one.
fn shape_radius(shape: usize, a: f32) -> f32 {
    let polygon = |n: f32| {
        let seg = TAU / n;
        let local = a.rem_euclid(seg) - seg * 0.5;
        (std::f32::consts::PI / n).cos() / local.cos()
    };
    match shape {
        1 => polygon(6.0),
        2 => polygon(8.0),
        3 => {
            // Five-pointed star: radius swings between 1 and 0.45.
            let seg = TAU / 5.0;
            let t = (a.rem_euclid(seg) / seg - 0.5).abs() * 2.0;
            0.45 + 0.55 * t * t
        }
        _ => 1.0,
    }
}

fn lens(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let radius = pct_h(img, p.f("radius"));
    if radius < 0.5 {
        return img.clone();
    }
    let shape = p.choice("shape");
    let rot = p.f("rotation").to_radians();
    let boost = p.f("boost");
    let threshold = p.f("threshold");
    // A golden-angle spiral covers the aperture evenly with few taps.
    let n = ((radius * radius * 0.8) as usize).clamp(12, 128);
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let offsets: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let r = ((i as f32 + 0.5) / n as f32).sqrt();
            let a = i as f32 * golden;
            let r = r * shape_radius(shape, a) * radius;
            // Oval: an anamorphic lens's tall bokeh, half as wide as high
            // before the shape's rotation.
            let squeeze = if shape == 4 { 0.5 } else { 1.0 };
            let (x, y) = (a.cos() * r * squeeze, a.sin() * r);
            let (s, c) = rot.sin_cos();
            (x * c - y * s, x * s + y * c)
        })
        .collect();
    Image::from_fn(img.width, img.height, |x, y| {
        let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut acc = [0.0f32; 4];
        let mut wsum = 0.0;
        for &(ox, oy) in &offsets {
            let s = img.sample(cx + ox, cy + oy, Edge::Clamp);
            // Bright points dominate their neighbourhood the way real
            // out-of-focus highlights do.
            let w = 1.0 + boost * smoothstep(threshold, threshold + 0.5, luma(s));
            for c in 0..4 {
                acc[c] += s[c] * w;
            }
            wsum += w;
        }
        scale_px(acc, 1.0 / wsum)
    })
}

fn saturate(px: Px, amount: f32) -> Px {
    let l = luma(px);
    [l + (px[0] - l) * (1.0 + amount), l + (px[1] - l) * (1.0 + amount), l + (px[2] - l) * (1.0 + amount), px[3]]
        .map(|v| v.max(0.0))
}

fn tilt_shift(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let r = pct_h(img, p.f("radius"));
    let full = gaussian_blur(img, r * 0.5, r * 0.5, Edge::Clamp);
    let half = gaussian_blur(img, r * 0.25, r * 0.25, Edge::Clamp);
    let (s, c) = p.f("angle").to_radians().sin_cos();
    let focus = p.f("focus");
    let half_width = p.f("width") * 0.5;
    let falloff = p.f("falloff").max(1e-3);
    let sat = p.f("saturation");
    let (w, h) = (img.width as f32, img.height as f32);
    let out = Image::from_fn(img.width, img.height, |x, y| {
        // Distance from the focus line, measured in frame heights.
        let (u, v) = ((x as f32 + 0.5 - w * 0.5) / h, (y as f32 + 0.5) / h - focus);
        let d = (v * c - u * s).abs();
        let m = smoothstep(half_width, half_width + falloff, d);
        let px = if m < 0.5 {
            crate::image::lerp_px(img.get(x, y), half.get(x, y), m * 2.0)
        } else {
            crate::image::lerp_px(half.get(x, y), full.get(x, y), m * 2.0 - 1.0)
        };
        saturate(px, sat)
    });
    apply_mix(img, out, p)
}

fn background_fill(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    // Bounding box of the visible picture.
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..img.height {
        for x in 0..img.width {
            if img.get(x, y)[3] > 0.5 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return img.clone();
    }
    let (bw, bh) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let (w, h) = (img.width as f32, img.height as f32);
    let s = (w / bw).max(h / bh) * p.f("zoom");
    let (bcx, bcy) = ((x0 + x1) as f32 * 0.5, (y0 + y1) as f32 * 0.5);
    let enlarged = crate::ops::warp(img, Edge::Clamp, |x, y| {
        let sx = bcx + (x - w * 0.5) / s;
        let sy = bcy + (y - h * 0.5) / s;
        (sx.clamp(x0 as f32 + 0.5, x1 as f32 - 0.5), sy.clamp(y0 as f32 + 0.5, y1 as f32 - 0.5))
    });
    let r = pct_h(img, p.f("blur")) * 0.5;
    let bg = gaussian_blur(&enlarged, r, r, Edge::Clamp);
    let dim = 1.0 - p.f("dim");
    let out = img.map_xy(|x, y, fg| {
        let b = bg.get(x, y);
        over(fg, [b[0] * dim, b[1] * dim, b[2] * dim, b[3]])
    });
    apply_mix(img, out, p)
}
