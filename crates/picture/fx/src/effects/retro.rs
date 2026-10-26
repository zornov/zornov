// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Retro: VHS, film, CRT, 8-bit dither, chromatic aberration, RGB split,
//! glitch.

use super::{apply_mix, Category, EffectContext, EffectDef, MIX};
use crate::image::{add_light, linear_to_srgb, premultiply, srgb_to_linear, unpremultiply, Edge, Image, Px};
use crate::math::{clamp01, hash1, hash2, hash3, noise1, smoothstep};
use crate::ops::{box_blur, gaussian_blur, mix, pct_h};
use crate::params::{ParamDef, ParamSet, Unit};

pub static EFFECTS: &[EffectDef] = &[
    EffectDef::new(
        "vhs",
        "VHS",
        Category::Retro,
        "Worn videotape: smeared colour, tracking noise and scanlines.",
        &[
            ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
            ParamDef::float("bleed", "Colour bleed", 0.0, 5.0, 0.8).unit(Unit::PercentHeight),
            ParamDef::float("tracking", "Tracking noise", 0.0, 1.0, 0.4).unit(Unit::Fraction),
            ParamDef::float("scanlines", "Scanlines", 0.0, 1.0, 0.35).unit(Unit::Fraction),
            ParamDef::float("noise", "Snow", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        vhs,
    )
    .animated()
    .identity("amount", 0.0),
    EffectDef::new(
        "film",
        "Old film",
        Category::Retro,
        "Projected film: gate weave, dust, scratches, flicker and grain.",
        &[
            ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
            ParamDef::float("weave", "Gate weave", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::float("dust", "Dust", 0.0, 1.0, 0.4).unit(Unit::Fraction),
            ParamDef::float("scratches", "Scratches", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::float("flicker", "Flicker", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::float("grain", "Grain", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::float("tint", "Age tint", 0.0, 1.0, 0.25).unit(Unit::Fraction),
            ParamDef::float("vignette", "Vignette", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        film,
    )
    .animated()
    .identity("amount", 0.0)
    .wgsl(crate::shaders::FILM),
    EffectDef::new(
        "crt",
        "CRT screen",
        Category::Retro,
        "Curved tube screen with scanlines and a phosphor mask.",
        &[
            ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
            ParamDef::float("curvature", "Curvature", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            ParamDef::float("scanlines", "Scanlines", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::float("mask", "Phosphor mask", 0.0, 1.0, 0.4).unit(Unit::Fraction),
            ParamDef::float("vignette", "Vignette", 0.0, 1.0, 0.4).unit(Unit::Fraction),
            ParamDef::float("glow", "Glow", 0.0, 1.0, 0.3).unit(Unit::Fraction),
        ],
        crt,
    )
    .identity("amount", 0.0),
    EffectDef::new(
        "dither_8bit",
        "8-bit dither",
        Category::Retro,
        "Chunky pixels and a small palette with ordered dithering.",
        &[
            ParamDef::float("pixel", "Pixel size", 0.0, 5.0, 0.5).unit(Unit::PercentHeight),
            ParamDef::choice(
                "palette",
                "Palette",
                &["Black & white", "Handheld green", "16 colours", "64 colours", "512 colours"],
                2,
            ),
            ParamDef::float("dither", "Dither", 0.0, 1.0, 1.0).unit(Unit::Fraction),
            MIX,
        ],
        dither,
    )
    .identity("mix", 0.0),
    EffectDef::new(
        "chromatic_aberration",
        "Chromatic aberration",
        Category::Retro,
        "Colour fringes growing towards the frame edges, like a cheap lens.",
        &[
            ParamDef::float("amount", "Amount", 0.0, 5.0, 0.6).unit(Unit::PercentHeight),
            ParamDef::point("center", "Centre", [0.5, 0.5]),
        ],
        chromatic,
    )
    .identity("amount", 0.0)
    .wgsl(crate::shaders::CHROMATIC_ABERRATION),
    EffectDef::new(
        "rgb_split",
        "RGB split",
        Category::Retro,
        "Red and blue copies pushed apart in one direction.",
        &[
            ParamDef::float("distance", "Distance", 0.0, 10.0, 1.0).unit(Unit::PercentHeight),
            ParamDef::float("angle", "Angle", -180.0, 180.0, 0.0).unit(Unit::Degrees),
        ],
        rgb_split,
    )
    .identity("distance", 0.0)
    .wgsl(crate::shaders::RGB_SPLIT),
    EffectDef::new(
        "glitch",
        "Glitch",
        Category::Retro,
        "Digital breakup: displaced blocks, shifted slices, split colour.",
        &[
            ParamDef::float("amount", "Amount", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::float("block", "Block size", 1.0, 20.0, 4.0).unit(Unit::PercentHeight),
            ParamDef::float("slices", "Slice shift", 0.0, 1.0, 0.6).unit(Unit::Fraction),
            ParamDef::float("rgb", "Colour split", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::float("rate", "Changes per second", 1.0, 30.0, 8.0).unit(Unit::Hertz),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        glitch,
    )
    .animated()
    .identity("amount", 0.0),
];

fn seed_of(ctx: &EffectContext, p: &ParamSet) -> u32 {
    ctx.seed ^ (p.i("seed") as u32).wrapping_mul(0x9e37_79b9)
}

/// Frame number at `fps`, used to step per-frame randomness.
fn frame_index(t: f32, fps: f32) -> i32 {
    (t * fps).floor() as i32
}

fn vhs(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = p.f("amount");
    let (w, h) = (img.width, img.height);
    let hf = h as f32;
    let seed = seed_of(ctx, p);
    let frame = frame_index(ctx.time, ctx.frame_rate.max(1.0));
    let t = ctx.time;
    let tracking = p.f("tracking");
    // Tracking: a band of torn lines rolls slowly down the frame, and every
    // line wobbles sideways a little.
    let band_y = ((t * 0.07 + hash1(0, seed)) % 1.0) * 1.3 - 0.15;
    let band_h = 0.06;
    let shifted = Image::from_fn(w, h, |x, y| {
        let v = (y as f32 + 0.5) / hf;
        let in_band = smoothstep(band_h, 0.0, (v - band_y).abs());
        let jitter = noise1(y as f32 * 0.35 + t * 40.0, seed) * 0.0015 * hf * tracking;
        let tear = in_band * tracking * (hash2(y as i32 / 2, frame, seed) - 0.3) * 0.04 * hf;
        img.sample(x as f32 + 0.5 - jitter - tear, y as f32 + 0.5, Edge::Clamp)
    });
    // Split into luma and chroma in an encoded space; tape kept far less
    // colour detail than brightness detail, so chroma gets smeared.
    let bleed = pct_h(img, p.f("bleed"));
    let ycc = shifted.map(|px| {
        let c = unpremultiply(px).map(|v| linear_to_srgb(v.max(0.0)));
        let y = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        [y * px[3], (c[2] - y) * px[3], (c[0] - y) * px[3], px[3]]
    });
    let chroma = box_blur(&ycc, bleed, 0.0, Edge::Clamp);
    let luma_soft = box_blur(&ycc, bleed * 0.15, 0.0, Edge::Clamp);
    let scan = p.f("scanlines");
    let snow = p.f("noise");
    let line_period = (hf / 240.0).max(2.0);
    let out = Image::from_fn(w, h, |x, y| {
        let yl = luma_soft.get(x, y);
        // Chroma lags behind luma, as on tape.
        let ch = chroma.sample(x as f32 + 0.5 - bleed * 0.5, y as f32 + 0.5, Edge::Clamp);
        let a = yl[3].max(1e-6);
        let (yy, cb, cr) = (yl[0] / a, ch[1] / ch[3].max(1e-6), ch[2] / ch[3].max(1e-6));
        let mut rgb = [yy + cr, yy - 0.194 * cb - 0.509 * cr, yy + cb];
        let v = (y as f32 + 0.5) / hf;
        let in_band = smoothstep(band_h, 0.0, (v - band_y).abs());
        let n = hash3(x as i32, y as i32, frame, seed);
        let noise = (n - 0.5) * snow * 0.25 + in_band * tracking * smoothstep(0.85, 1.0, n) * 0.8;
        let phase = (y as f32 / line_period).fract();
        let line = 1.0 - scan * 0.35 * smoothstep(0.3, 0.7, phase);
        for c in &mut rgb {
            // Slightly lifted blacks and softened whites.
            *c = (0.04 + *c * 0.92 + noise) * line;
        }
        let lin = rgb.map(|v| srgb_to_linear(clamp01(v)));
        premultiply([lin[0] * 0.9 + lin[1] * 0.1, lin[1], lin[2] * 0.9 + lin[1] * 0.1], yl[3])
    });
    mix(img, &out, amount)
}

fn film(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = p.f("amount");
    let (w, h) = (img.width as f32, img.height as f32);
    let seed = seed_of(ctx, p);
    // Damage changes at film speed, whatever the sequence rate.
    let frame = frame_index(ctx.time, 24.0);
    let weave = p.f("weave");
    let (wx, wy) = (
        noise1(ctx.time * 3.0, seed) * 0.002 * h * weave,
        noise1(ctx.time * 2.3 + 7.0, seed ^ 1) * 0.004 * h * weave,
    );
    let flicker = 1.0 + p.f("flicker") * 0.12 * (hash1(frame, seed ^ 2) - 0.5) * 2.0;
    let grain = p.f("grain");
    let tint = p.f("tint");
    let vignette = p.f("vignette");
    // Dust specks for this frame: position, radius, dark or light.
    let specks: Vec<(f32, f32, f32, bool)> = (0..(p.f("dust") * 40.0) as i32)
        .map(|i| {
            let r = |k: i32| hash2(i, frame * 7 + k, seed ^ 3);
            (r(0) * w, r(1) * h, (0.0008 + 0.003 * r(2) * r(2)) * h, r(3) < 0.75)
        })
        .collect();
    // A few long scratches that drift slowly and pop in and out.
    let scratches: Vec<(f32, f32)> = (0..3)
        .filter_map(|i| {
            let alive = hash1(frame / 6 + i * 31, seed ^ 4) < p.f("scratches") * 0.8;
            alive.then(|| {
                let x = (hash1(i, seed ^ 5) + noise1(ctx.time * 0.4 + i as f32 * 9.0, seed ^ 6) * 0.05).rem_euclid(1.0) * w;
                (x, 0.4 + 0.6 * hash1(frame + i, seed ^ 7))
            })
        })
        .collect();
    let out = img.map_xy(|x, y, _| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let px = img.sample(fx + wx, fy + wy, Edge::Clamp);
        let mut c = unpremultiply(px).map(|v| linear_to_srgb(v.max(0.0)));
        // Ageing: fade towards warm, slightly desaturated tones.
        let l = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        let aged = [l * 1.07 + 0.03, l * 0.97 + 0.02, l * 0.82];
        for k in 0..3 {
            c[k] = c[k] + (aged[k] - c[k]) * tint * 0.7;
        }
        let g = (hash3(x as i32, y as i32, frame, seed ^ 8) - 0.5) * grain * 0.16;
        let (u, v) = (fx / w - 0.5, fy / h - 0.5);
        let vig = 1.0 - vignette * smoothstep(0.2, 0.75, (u * u + v * v).sqrt() * 1.2);
        for k in 0..3 {
            c[k] = (c[k] * flicker + g) * vig;
        }
        for &(sx, sy, r, dark) in &specks {
            let d = ((fx - sx).powi(2) + (fy - sy).powi(2)).sqrt();
            let k = smoothstep(r, r * 0.4, d);
            if k > 0.0 {
                let target = if dark { 0.05 } else { 0.95 };
                for ch in &mut c {
                    *ch += (target - *ch) * k * 0.85;
                }
            }
        }
        for &(sx, bright) in &scratches {
            let k = smoothstep(0.0015 * h, 0.0, (fx - sx).abs()) * bright;
            for ch in &mut c {
                *ch += (0.9 - *ch) * k * 0.6;
            }
        }
        premultiply(c.map(|v| srgb_to_linear(clamp01(v))), px[3])
    });
    mix(img, &out, amount)
}

fn crt(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = p.f("amount");
    let (w, h) = (img.width as f32, img.height as f32);
    let curv = p.f("curvature") * 0.25;
    let scan = p.f("scanlines");
    let mask = p.f("mask");
    let vignette = p.f("vignette");
    let glow_amt = p.f("glow");
    let line_period = (h / 270.0).max(3.0);
    let r = 0.004 * h;
    let glow = gaussian_blur(img, r * 2.0, r * 2.0, Edge::Clamp);
    let out = Image::from_fn(img.width, img.height, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / w * 2.0 - 1.0, (y as f32 + 0.5) / h * 2.0 - 1.0);
        // Barrel curvature: points further out are pushed further out.
        let k = 1.0 + curv * (u * u + v * v);
        let (su, sv) = (u * k, v * k);
        if su.abs() > 1.0 || sv.abs() > 1.0 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        let (sx, sy) = ((su + 1.0) * 0.5 * w, (sv + 1.0) * 0.5 * h);
        let px = img.sample(sx, sy, Edge::Clamp);
        let g = glow.sample(sx, sy, Edge::Clamp);
        let phase = (sy / line_period).fract();
        let beam = 1.0 - scan * 0.6 * (1.0 - (phase * std::f32::consts::PI).sin());
        let triad = x % 3;
        let edge_fade = smoothstep(1.0, 0.97, su.abs()) * smoothstep(1.0, 0.97, sv.abs());
        let vig = (1.0 - vignette * 0.6 * (su * su + sv * sv) * 0.5) * edge_fade;
        let mut out = [0.0f32; 4];
        for c in 0..3 {
            let m = if c == triad { 1.0 } else { 1.0 - mask * 0.55 };
            // Mask and scanlines darken; compensate so the overall level holds.
            let gain = 1.0 + mask * 0.35 + scan * 0.25;
            out[c] = (px[c] * beam * m * gain + g[c] * glow_amt * 0.5) * vig;
        }
        out[3] = 1.0;
        out
    });
    mix(img, &out, amount)
}

/// 8x8 ordered-dither threshold matrix, built from the recursive Bayer
/// pattern, as values in -0.5..0.5.
fn bayer8(x: usize, y: usize) -> f32 {
    let mut v = 0;
    let (mut xx, mut yy) = (x & 7, y & 7);
    for bit in (0..3).rev() {
        let (bx, by) = ((xx >> bit) & 1, (yy >> bit) & 1);
        v = v * 4 + [[0, 2], [3, 1]][by][bx];
        xx &= (1 << bit) - 1;
        yy &= (1 << bit) - 1;
    }
    (v as f32 + 0.5) / 64.0 - 0.5
}

/// Our 16-colour palette in sRGB encoding: a balanced spread of darks,
/// mids, skin tones, foliage, sky and accents.
const PALETTE16: [[f32; 3]; 16] = [
    [0.05, 0.04, 0.08],
    [0.98, 0.97, 0.93],
    [0.45, 0.45, 0.50],
    [0.75, 0.75, 0.78],
    [0.55, 0.16, 0.18],
    [0.90, 0.36, 0.28],
    [0.95, 0.72, 0.55],
    [0.98, 0.85, 0.30],
    [0.20, 0.42, 0.18],
    [0.52, 0.78, 0.32],
    [0.12, 0.22, 0.45],
    [0.30, 0.55, 0.88],
    [0.60, 0.85, 0.95],
    [0.42, 0.24, 0.55],
    [0.85, 0.45, 0.70],
    [0.40, 0.27, 0.15],
];

const GREEN4: [[f32; 3]; 4] = [[0.06, 0.22, 0.06], [0.19, 0.38, 0.19], [0.55, 0.67, 0.06], [0.61, 0.74, 0.06]];
const MONO: [[f32; 3]; 2] = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];

fn nearest(c: [f32; 3], pal: &[[f32; 3]]) -> [f32; 3] {
    *pal.iter()
        .min_by(|a, b| {
            let d = |q: &[f32; 3]| {
                // Weighted distance; green differences are the most visible.
                0.3 * (q[0] - c[0]).powi(2) + 0.59 * (q[1] - c[1]).powi(2) + 0.11 * (q[2] - c[2]).powi(2)
            };
            d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap()
}

fn dither(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let cell = pct_h(img, p.f("pixel")).max(1.0);
    let palette = p.choice("palette");
    let strength = p.f("dither");
    let out = img.map_xy(|x, y, _| {
        let (cx, cy) = ((x as f32 / cell).floor(), (y as f32 / cell).floor());
        let px = img.sample((cx + 0.5) * cell, (cy + 0.5) * cell, Edge::Clamp);
        let c = unpremultiply(px).map(|v| clamp01(linear_to_srgb(v.max(0.0))));
        let t = bayer8(cx as usize, cy as usize) * strength;
        let q = match palette {
            0 => {
                let l = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
                nearest([l + t; 3], &MONO)
            }
            1 => {
                let l = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
                let i = ((l + t / 3.0) * 4.0).floor().clamp(0.0, 3.0) as usize;
                GREEN4[i]
            }
            2 => nearest(c.map(|v| v + t * 0.35), &PALETTE16),
            n => {
                let levels = if n == 3 { 3.0 } else { 7.0 };
                c.map(|v| ((v + t / levels) * levels).round().clamp(0.0, levels) / levels)
            }
        };
        premultiply(q.map(srgb_to_linear), px[3])
    });
    apply_mix(img, out, p)
}

fn chromatic(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let (w, h) = (img.width as f32, img.height as f32);
    let k = pct_h(img, p.f("amount")) / (0.5 * (w * w + h * h).sqrt());
    let c = p.point("center");
    let (cx, cy) = (c[0] * w, c[1] * h);
    Image::from_fn(img.width, img.height, |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let r = img.sample(cx + dx * (1.0 - k), cy + dy * (1.0 - k), Edge::Clamp);
        let g = img.sample(cx + dx, cy + dy, Edge::Clamp);
        let b = img.sample(cx + dx * (1.0 + k), cy + dy * (1.0 + k), Edge::Clamp);
        split_px(r, g, b)
    })
}

/// Combines channels taken from three samples. Alpha is the largest of the
/// three so premultiplied colour never exceeds it.
fn split_px(r: Px, g: Px, b: Px) -> Px {
    [r[0], g[1], b[2], r[3].max(g[3]).max(b[3])]
}

fn rgb_split(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let d = pct_h(img, p.f("distance"));
    let (s, c) = p.f("angle").to_radians().sin_cos();
    let (dx, dy) = (c * d * 0.5, s * d * 0.5);
    Image::from_fn(img.width, img.height, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        split_px(
            img.sample(fx - dx, fy - dy, Edge::Clamp),
            img.get(x, y),
            img.sample(fx + dx, fy + dy, Edge::Clamp),
        )
    })
}

fn glitch(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = p.f("amount");
    if amount <= 0.0 {
        return img.clone();
    }
    let (w, h) = (img.width as f32, img.height as f32);
    let seed = seed_of(ctx, p);
    let step = frame_index(ctx.time, p.f("rate"));
    let block = pct_h(img, p.f("block")).max(2.0);
    let slices = p.f("slices");
    let rgb = p.f("rgb");
    // Slice bands: rows grouped into bands of random height.
    let band_of = |y: f32| -> (i32, f32) {
        let b = (y / (block * 0.5)).floor() as i32;
        let r = hash2(b, step, seed ^ 0x11);
        (b, r)
    };
    img.map_xy(|x, y, _| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let (band, r) = band_of(fy);
        let mut sx = fx;
        let mut sy = fy;
        let mut split = 0.0;
        if r < amount * 0.45 * slices {
            let shift = (hash2(band, step, seed ^ 0x22) - 0.5) * 0.3 * w * amount;
            sx += shift;
            split = rgb;
        }
        let (bx, by) = ((fx / block).floor() as i32, (fy / block).floor() as i32);
        if hash3(bx, by, step, seed ^ 0x33) < amount * 0.12 {
            // Copy this block from somewhere else in the frame.
            sx += (hash3(bx, by, step, seed ^ 0x44) - 0.5) * w * 0.4;
            sy += (hash3(bx, by, step, seed ^ 0x55) - 0.5) * h * 0.1;
            split = rgb;
        }
        let o = split * block * 0.4;
        let px = split_px(
            img.sample(sx - o, sy, Edge::Wrap),
            img.sample(sx, sy, Edge::Wrap),
            img.sample(sx + o, sy, Edge::Wrap),
        );
        if split > 0.0 && hash3(bx, by, step, seed ^ 0x66) < 0.05 * amount {
            // The odd block flashes bright, like corrupted data.
            add_light(px, [0.3, 0.3, 0.35])
        } else {
            px
        }
    })
}
