// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Light: drop shadow, rim light, glow, bloom, light leak, lens flare,
//! god rays, neon edges.

use super::{apply_mix, Category, EffectContext, EffectDef, MIX};
use crate::image::{add_light, luma, Edge, Image, Px};
use crate::math::{fbm, hsv_to_rgb, noise1, smoothstep};
use crate::ops::{gaussian_blur, pct_h};
use crate::params::{ParamDef, ParamSet, Unit};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const WARM: [f32; 4] = [1.0, 0.82, 0.62, 1.0];

pub static EFFECTS: &[EffectDef] = &[
    EffectDef::new(
        "drop_shadow",
        "Drop shadow",
        Category::Light,
        "A soft shadow under the layer, falling away from the light: lifts titles, cards and cut-outs off what is behind them.",
        &[
            ParamDef::float("distance", "Distance", 0.0, 10.0, 1.0).unit(Unit::PercentHeight),
            ParamDef::float("angle", "Angle", 0.0, 360.0, 90.0).unit(Unit::Degrees),
            ParamDef::float("softness", "Softness", 0.0, 10.0, 2.0).unit(Unit::PercentHeight),
            ParamDef::float("opacity", "Opacity", 0.0, 1.0, 0.45).unit(Unit::Fraction),
            ParamDef::colour("colour", "Colour", [0.0, 0.0, 0.0, 1.0]),
        ],
        drop_shadow,
    )
    .identity("opacity", 0.0),
    EffectDef::new(
        "rim_light",
        "Rim light",
        Category::Light,
        "Light catching the edges of a cut-out that face the light: sets a character into the light of the scene behind it.",
        &[
            ParamDef::float("angle", "Light from", 0.0, 360.0, 200.0).unit(Unit::Degrees),
            ParamDef::float("width", "Width", 0.0, 5.0, 0.8).unit(Unit::PercentHeight),
            ParamDef::float("intensity", "Intensity", 0.0, 2.0, 0.5),
            ParamDef::float("softness", "Softness", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::colour("colour", "Colour", WARM),
        ],
        rim_light,
    )
    .identity("intensity", 0.0),
    EffectDef::new(
        "glow",
        "Glow",
        Category::Light,
        "Soft light spilling from the bright parts of the picture.",
        &[
            ParamDef::float("threshold", "Threshold", 0.0, 2.0, 0.6),
            ParamDef::float("radius", "Radius", 0.0, 20.0, 3.0).unit(Unit::PercentHeight),
            ParamDef::float("intensity", "Intensity", 0.0, 4.0, 1.0),
            ParamDef::colour("tint", "Tint", WHITE),
        ],
        glow,
    )
    .identity("intensity", 0.0),
    EffectDef::new(
        "bloom",
        "Bloom",
        Category::Light,
        "Wide, layered halo around highlights, like light scattering in a lens.",
        &[
            ParamDef::float("threshold", "Threshold", 0.0, 2.0, 0.8),
            ParamDef::float("radius", "Radius", 0.0, 30.0, 6.0).unit(Unit::PercentHeight),
            ParamDef::float("intensity", "Intensity", 0.0, 4.0, 0.6),
            ParamDef::float("warmth", "Warmth", -1.0, 1.0, 0.15),
        ],
        bloom,
    )
    .identity("intensity", 0.0),
    EffectDef::new(
        "light_leak",
        "Light leak",
        Category::Light,
        "Drifting warm glows bleeding in from the frame edges.",
        &[
            ParamDef::float("intensity", "Intensity", 0.0, 2.0, 0.6),
            ParamDef::float("hue", "Hue", 0.0, 360.0, 25.0).unit(Unit::Degrees),
            ParamDef::float("spread", "Hue spread", 0.0, 180.0, 30.0).unit(Unit::Degrees),
            ParamDef::float("speed", "Speed", 0.0, 2.0, 0.25),
            ParamDef::float("scale", "Size", 0.2, 3.0, 1.0).unit(Unit::Times),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        light_leak,
    )
    .animated()
    .identity("intensity", 0.0),
    EffectDef::new(
        "lens_flare",
        "Lens flare",
        Category::Light,
        "A bright source with a halo, a streak and ghost reflections.",
        &[
            ParamDef::point("position", "Light position", [0.28, 0.25]),
            ParamDef::float("intensity", "Intensity", 0.0, 4.0, 1.0),
            ParamDef::float("size", "Size", 0.2, 3.0, 1.0).unit(Unit::Times),
            ParamDef::colour("tint", "Tint", WARM),
            ParamDef::float("streak", "Streak", 0.0, 2.0, 0.6),
            ParamDef::int("ghosts", "Ghosts", 0, 8, 4),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        lens_flare,
    )
    .animated()
    .identity("intensity", 0.0),
    EffectDef::new(
        "god_rays",
        "God rays",
        Category::Light,
        "Shafts of light streaming from bright areas towards the viewer.",
        &[
            ParamDef::point("center", "Source", [0.5, 0.15]),
            ParamDef::float("length", "Length", 0.0, 1.0, 0.45).unit(Unit::Fraction),
            ParamDef::float("intensity", "Intensity", 0.0, 4.0, 1.0),
            ParamDef::float("threshold", "Threshold", 0.0, 2.0, 0.6),
            ParamDef::colour("tint", "Tint", WARM),
        ],
        god_rays,
    )
    .identity("intensity", 0.0),
    EffectDef::new(
        "neon_edge",
        "Neon edges",
        Category::Light,
        "Outlines traced in glowing neon tubes.",
        &[
            ParamDef::float("width", "Line width", 0.0, 2.0, 0.25).unit(Unit::PercentHeight),
            ParamDef::float("glow", "Glow radius", 0.0, 10.0, 1.5).unit(Unit::PercentHeight),
            ParamDef::float("intensity", "Intensity", 0.0, 4.0, 1.5),
            ParamDef::float("hue", "Hue", 0.0, 360.0, 300.0).unit(Unit::Degrees),
            ParamDef::bool("rainbow", "Rainbow", true),
            ParamDef::float("background", "Keep picture", 0.0, 1.0, 0.15).unit(Unit::Fraction),
            MIX,
        ],
        neon_edge,
    )
    .identity("mix", 0.0),
];

/// The layer's own shape, blurred, offset along `angle` (clockwise from
/// pointing right, so 90 falls straight down) and tinted, drawn under it.
/// Three layers, as real shadows are and as "smooth shadows" are made:
/// a tight dark one where the layer meets the ground, a softer one, and a
/// wide faint one, together falling off with no edge.
fn drop_shadow(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let d = pct_h(img, p.f("distance"));
    let (s, c) = p.f("angle").to_radians().sin_cos();
    let soft = pct_h(img, p.f("softness"));
    let alpha = img.map(|q| [0.0, 0.0, 0.0, q[3]]);
    // (share of the distance, share of the softness, strength)
    let layers: Vec<(f32, f32, Image)> = [(0.25, 0.2, 0.55), (0.6, 0.55, 0.35), (1.25, 1.3, 0.25)]
        .iter()
        .map(|&(dk, sk, a)| {
            let sigma = (soft * sk * 0.5).max(0.3);
            (dk, a, gaussian_blur(&alpha, sigma, sigma, Edge::Transparent))
        })
        .collect();
    let col = p.colour("colour");
    let strength = p.f("opacity") * col[3];
    img.map_xy(|x, y, q| {
        // Shadows overlap as light is blocked: one minus what gets through.
        let through: f32 = layers.iter().map(|(dk, a, im)| 1.0 - im.sample(x as f32 + 0.5 - c * d * dk, y as f32 + 0.5 - s * d * dk, Edge::Transparent)[3] * a).product();
        let a = (1.0 - through) / (1.0 - 0.45 * 0.65 * 0.75) * strength;
        let under = 1.0 - q[3];
        [q[0] + col[0] * a * under, q[1] + col[1] * a * under, q[2] + col[2] * a * under, q[3] + a * under]
    })
}

/// The edges facing the light (`angle`, clockwise from pointing right, is
/// where the light comes from) lit in its colour: a pixel is on the rim
/// when the picture is see-through a little way towards the light. Only
/// what is already there brightens; nothing is added outside the shape.
fn rim_light(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let w = pct_h(img, p.f("width")).max(0.5);
    let (s, c) = p.f("angle").to_radians().sin_cos();
    let alpha = img.map(|q| [0.0, 0.0, 0.0, q[3]]);
    let sigma = (w * 0.5).max(0.3);
    let soft = gaussian_blur(&alpha, sigma, sigma, Edge::Transparent);
    let rim = img.map_xy(|x, y, q| {
        let ahead = soft.sample(x as f32 + 0.5 + c * w, y as f32 + 0.5 + s * w, Edge::Transparent)[3];
        let r = (q[3] - ahead).max(0.0);
        [r, r, r, r]
    });
    let k = p.f("softness").clamp(0.0, 1.0) * w;
    let rim = if k > 0.3 { gaussian_blur(&rim, k, k, Edge::Transparent) } else { rim };
    let col = tint3(p.colour("colour"));
    let i = p.f("intensity");
    img.map_xy(|x, y, q| {
        // Kept inside the shape, and scaled by how solid it is there.
        let r = rim.get(x, y)[3].min(1.0) * q[3] * i;
        add_light(q, [col[0] * r, col[1] * r, col[2] * r])
    })
}

fn tint3(c: [f32; 4]) -> [f32; 3] {
    [c[0] * c[3], c[1] * c[3], c[2] * c[3]]
}

/// The part of the picture brighter than `threshold`, with a soft knee so
/// highlights do not pop in and out as they cross it.
fn bright_pass(img: &Image, threshold: f32) -> Image {
    img.map(|p| {
        let k = smoothstep(threshold, threshold + 0.25, luma(p));
        [p[0] * k, p[1] * k, p[2] * k, 0.0]
    })
}

fn add_image(img: &Image, light: &Image, gain: [f32; 3]) -> Image {
    img.map_xy(|x, y, p| {
        let l = light.get(x, y);
        add_light(p, [l[0] * gain[0], l[1] * gain[1], l[2] * gain[2]])
    })
}

fn glow(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let r = pct_h(img, p.f("radius")) * 0.5;
    let bright = gaussian_blur(&bright_pass(img, p.f("threshold")), r, r, Edge::Clamp);
    let t = tint3(p.colour("tint"));
    let i = p.f("intensity");
    add_image(img, &bright, [t[0] * i, t[1] * i, t[2] * i])
}

fn bloom(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let bright = bright_pass(img, p.f("threshold"));
    let r = pct_h(img, p.f("radius"));
    // Several radii summed give the long tail real scattering has.
    let mut acc = Image::new(img.width, img.height);
    for (frac, weight) in [(0.06, 0.35), (0.15, 0.3), (0.35, 0.2), (0.7, 0.15)] {
        let b = gaussian_blur(&bright, r * frac, r * frac, Edge::Clamp);
        for (a, s) in acc.pixels.iter_mut().zip(&b.pixels) {
            for c in 0..3 {
                a[c] += s[c] * weight;
            }
        }
    }
    let i = p.f("intensity");
    let w = p.f("warmth") * 0.25;
    add_image(img, &acc, [i * (1.0 + w), i, i * (1.0 - w)])
}

/// Screen-like addition: light is reduced where the picture is already
/// bright, which keeps leaks from clipping everything to white.
fn screen_light(p: Px, l: [f32; 3]) -> Px {
    let soft = |c: f32, l: f32| l * (1.0 - c.clamp(0.0, 1.0));
    add_light(p, [soft(p[0], l[0]), soft(p[1], l[1]), soft(p[2], l[2])])
}

fn light_leak(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let seed = ctx.seed ^ (p.i("seed") as u32).wrapping_mul(0x9e37_79b9);
    let t = ctx.time * p.f("speed");
    let scale = p.f("scale");
    let intensity = p.f("intensity");
    let hue = p.f("hue");
    let spread = p.f("spread");
    let aspect = img.width as f32 / img.height as f32;
    // Three blobs wander around the left/right edges on slow noise paths.
    let blobs: Vec<([f32; 2], f32, [f32; 3])> = (0..3)
        .map(|i| {
            let s = seed.wrapping_add(i * 7919);
            let side = if i % 2 == 0 { -0.1 } else { aspect + 0.1 };
            let x = side + noise1(t * 0.7 + i as f32 * 3.1, s) * 0.35 * aspect;
            let y = 0.5 + noise1(t * 0.5 + 11.0, s ^ 0x55) * 0.6;
            let size = (0.35 + 0.25 * noise1(t * 0.3 + 5.0, s ^ 0xaa).abs()) * scale;
            let h = hue + spread * (i as f32 - 1.0);
            (
                [x, y],
                size,
                hsv_to_rgb(h, 0.85, 1.0),
            )
        })
        .collect();
    img.map_xy(|x, y, px| {
        let (u, v) = ((x as f32 + 0.5) / img.height as f32, (y as f32 + 0.5) / img.height as f32);
        let mut l = [0.0f32; 3];
        for (c, size, col) in &blobs {
            // Stretch vertically: leaks come in as tall bands.
            let d2 = ((u - c[0]) / size).powi(2) + ((v - c[1]) / (size * 2.2)).powi(2);
            let k = (-d2 * 2.0).exp();
            for i in 0..3 {
                l[i] += col[i] * k;
            }
        }
        // Grainy texture so the leak looks like light on film, not a gradient.
        let tex = 0.75 + 0.5 * fbm(u * 3.0 / scale, v * 3.0 / scale, t * 0.5, 3, seed);
        for v in &mut l {
            *v *= intensity * tex;
        }
        screen_light(px, l)
    })
}

fn lens_flare(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let h = img.height as f32;
    let aspect = img.width as f32 / h;
    let pos = p.point("position");
    let light = [pos[0] * aspect, pos[1]];
    let center = [aspect * 0.5, 0.5];
    let size = p.f("size");
    let intensity = p.f("intensity");
    let tint = tint3(p.colour("tint"));
    let streak = p.f("streak");
    let seed = ctx.seed ^ p.i("seed") as u32;
    let ghosts: Vec<([f32; 2], f32, [f32; 3], f32)> = (0..p.i("ghosts"))
        .map(|i| {
            let r = |k: u32| crate::math::hash1(i, seed.wrapping_add(k));
            // Ghosts sit on the line from the light through the frame centre.
            let k = 0.4 + 1.8 * (i as f32 + r(1)) / p.i("ghosts").max(1) as f32;
            let at = [light[0] + (center[0] - light[0]) * k, light[1] + (center[1] - light[1]) * k];
            let radius = (0.02 + 0.07 * r(2)) * size;
            let colour = hsv_to_rgb(20.0 + 200.0 * r(3), 0.6, 1.0);
            (at, radius, colour, 0.06 + 0.12 * r(4))
        })
        .collect();
    img.map_xy(|x, y, px| {
        let (u, v) = ((x as f32 + 0.5) / h, (y as f32 + 0.5) / h);
        let (dx, dy) = (u - light[0], v - light[1]);
        let d = (dx * dx + dy * dy).sqrt();
        let core = 2.0 * (-(d / (0.025 * size)).powi(2)).exp() + 0.35 * (-d / (0.12 * size)).exp();
        let ring = 0.12 * (-((d - 0.22 * size) / (0.012 * size)).powi(2)).exp();
        let line = streak * (-(dy.abs() / (0.004 * size))).exp() * (-(dx.abs() / (0.6 * size))).exp();
        let mut l = [0.0f32; 3];
        for c in 0..3 {
            // The ring is slightly larger in red than blue, like dispersion.
            let ring_c = 0.12 * (-((d - 0.22 * size * (1.04 - 0.04 * c as f32)) / (0.012 * size)).powi(2)).exp();
            l[c] = tint[c] * (core + line) + ring_c * 0.6 + ring * 0.4 * tint[c];
        }
        for (at, radius, colour, strength) in &ghosts {
            let gd = ((u - at[0]).powi(2) + (v - at[1]).powi(2)).sqrt();
            // Soft-edged disc that is a little brighter at its rim.
            let disc = smoothstep(*radius, radius * 0.85, gd) * (0.6 + 0.4 * (gd / radius).min(1.0));
            for c in 0..3 {
                l[c] += colour[c] * disc * strength;
            }
        }
        add_light(px, l.map(|v| v * intensity))
    })
}

fn god_rays(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let intensity = p.f("intensity");
    let len = p.f("length");
    let bright = bright_pass(img, p.f("threshold"));
    let c = p.point("center");
    let (cx, cy) = (c[0] * img.width as f32, c[1] * img.height as f32);
    let tint = tint3(p.colour("tint"));
    const TAPS: usize = 48;
    img.map_xy(|x, y, px| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut acc = [0.0f32; 3];
        let mut weight = 1.0;
        let mut norm = 0.0;
        // March towards the source, collecting bright samples with decay,
        // so light from behind an object streams past its edges.
        for i in 0..TAPS {
            let t = i as f32 / TAPS as f32 * len;
            let s = bright.sample(fx + (cx - fx) * t, fy + (cy - fy) * t, Edge::Clamp);
            for k in 0..3 {
                acc[k] += s[k] * weight;
            }
            norm += weight;
            weight *= 0.96;
        }
        let g = intensity / norm.max(1e-6);
        add_light(px, [acc[0] * g * tint[0], acc[1] * g * tint[1], acc[2] * g * tint[2]])
    })
}

fn neon_edge(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let width = pct_h(img, p.f("width")).max(0.5);
    let soft = gaussian_blur(img, width * 0.5, width * 0.5, Edge::Clamp);
    let lum = |x: isize, y: isize| luma(soft.fetch(x, y, Edge::Clamp)).max(0.0).sqrt();
    let hue = p.f("hue");
    let rainbow = p.b("rainbow");
    let lines = Image::from_fn(img.width, img.height, |x, y| {
        let (x, y) = (x as isize, y as isize);
        let gx = lum(x + 1, y - 1) + 2.0 * lum(x + 1, y) + lum(x + 1, y + 1)
            - lum(x - 1, y - 1)
            - 2.0 * lum(x - 1, y)
            - lum(x - 1, y + 1);
        let gy = lum(x - 1, y + 1) + 2.0 * lum(x, y + 1) + lum(x + 1, y + 1)
            - lum(x - 1, y - 1)
            - 2.0 * lum(x, y - 1)
            - lum(x + 1, y - 1);
        // Gradients shrink as the line width grows, so scale them back up.
        let e = smoothstep(0.08, 0.35, (gx * gx + gy * gy).sqrt() * width.sqrt());
        let h = if rainbow { hue + gy.atan2(gx).to_degrees() } else { hue };
        let c = hsv_to_rgb(h, 0.8, 1.0);
        [c[0] * e, c[1] * e, c[2] * e, e]
    });
    let gr = pct_h(img, p.f("glow")) * 0.5;
    let halo = gaussian_blur(&lines, gr, gr, Edge::Clamp);
    let intensity = p.f("intensity");
    let keep = p.f("background");
    let out = img.map_xy(|x, y, px| {
        let (l, g) = (lines.get(x, y), halo.get(x, y));
        let base = [px[0] * keep, px[1] * keep, px[2] * keep, px[3]];
        add_light(base, [0, 1, 2].map(|c| l[c] + g[c] * intensity))
    });
    apply_mix(img, out, p)
}
