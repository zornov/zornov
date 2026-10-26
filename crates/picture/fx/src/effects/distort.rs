// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Distort: wave, ripple, twirl, bulge/pinch, fisheye/defish, glass, noise
//! displacement, camera shake.

use super::{Category, EffectContext, EffectDef};
use crate::image::{Edge, Image};
use crate::math::{fbm, hash2, noise1, smoothstep, value_noise, TAU};
use crate::ops::{pct_h, warp};
use crate::params::{ParamDef, ParamSet, Unit};

pub static EFFECTS: &[EffectDef] = &[
    EffectDef::new(
        "wave",
        "Wave",
        Category::Distort,
        "Sideways sine waves rolling through the picture.",
        &[
            ParamDef::float("amplitude", "Amplitude", 0.0, 20.0, 1.5).unit(Unit::PercentHeight),
            ParamDef::float("wavelength", "Wavelength", 1.0, 200.0, 25.0).unit(Unit::PercentHeight),
            ParamDef::float("angle", "Direction", -180.0, 180.0, 90.0).unit(Unit::Degrees),
            ParamDef::float("speed", "Speed", -5.0, 5.0, 0.5).unit(Unit::Hertz),
            ParamDef::float("phase", "Phase", 0.0, 360.0, 0.0).unit(Unit::Degrees),
        ],
        wave,
    )
    .animated()
    .identity("amplitude", 0.0)
    .wgsl(crate::shaders::WAVE),
    EffectDef::new(
        "ripple",
        "Ripple",
        Category::Distort,
        "Rings spreading out like a drop in water.",
        &[
            ParamDef::point("center", "Centre", [0.5, 0.5]),
            ParamDef::float("amplitude", "Amplitude", 0.0, 10.0, 1.0).unit(Unit::PercentHeight),
            ParamDef::float("wavelength", "Wavelength", 1.0, 50.0, 6.0).unit(Unit::PercentHeight),
            ParamDef::float("speed", "Speed", -5.0, 5.0, 1.0).unit(Unit::Hertz),
            ParamDef::float("decay", "Fade with distance", 0.0, 10.0, 2.0),
        ],
        ripple,
    )
    .animated()
    .identity("amplitude", 0.0)
    .wgsl(crate::shaders::RIPPLE),
    EffectDef::new(
        "twirl",
        "Twirl",
        Category::Distort,
        "Spins the centre more than the edges.",
        &[
            ParamDef::float("angle", "Angle", -720.0, 720.0, 120.0).unit(Unit::Degrees),
            ParamDef::float("radius", "Radius", 0.0, 150.0, 45.0).unit(Unit::PercentHeight),
            ParamDef::point("center", "Centre", [0.5, 0.5]),
        ],
        twirl,
    )
    .identity("angle", 0.0)
    .wgsl(crate::shaders::TWIRL),
    EffectDef::new(
        "bulge",
        "Bulge / pinch",
        Category::Distort,
        "Magnifies (positive) or squeezes (negative) a round area.",
        &[
            ParamDef::float("amount", "Amount", -1.0, 1.0, 0.5),
            ParamDef::float("radius", "Radius", 1.0, 150.0, 40.0).unit(Unit::PercentHeight),
            ParamDef::point("center", "Centre", [0.5, 0.5]),
        ],
        bulge,
    )
    .identity("amount", 0.0)
    .wgsl(crate::shaders::BULGE),
    EffectDef::new(
        "fisheye",
        "Fisheye / defish",
        Category::Distort,
        "Wide-lens barrel distortion, or (negative) its correction.",
        &[
            ParamDef::float("amount", "Amount", -1.0, 1.0, 0.5),
            ParamDef::float("zoom", "Zoom", 0.5, 2.0, 1.0).unit(Unit::Times),
        ],
        fisheye,
    )
    .identity("amount", 0.0)
    .wgsl(crate::shaders::FISHEYE),
    EffectDef::new(
        "glass",
        "Glass",
        Category::Distort,
        "Looking through frosted, reeded or bumpy glass.",
        &[
            ParamDef::float("distortion", "Distortion", 0.0, 5.0, 1.0).unit(Unit::PercentHeight),
            ParamDef::float("scale", "Pattern size", 0.5, 20.0, 3.0).unit(Unit::PercentHeight),
            ParamDef::choice("mode", "Glass", &["Frosted", "Reeded", "Tiles"], 1),
        ],
        glass,
    )
    .identity("distortion", 0.0),
    EffectDef::new(
        "noise_displace",
        "Noise displace",
        Category::Distort,
        "Organic, flowing warp driven by noise.",
        &[
            ParamDef::float("amount", "Amount", 0.0, 20.0, 2.0).unit(Unit::PercentHeight),
            ParamDef::float("scale", "Scale", 1.0, 200.0, 25.0).unit(Unit::PercentHeight),
            ParamDef::float("speed", "Speed", 0.0, 5.0, 0.5),
            ParamDef::int("octaves", "Detail", 1, 6, 3),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        noise_displace,
    )
    .animated()
    .identity("amount", 0.0),
    EffectDef::new(
        "water",
        "Living water",
        Category::Distort,
        "Ripples that flow one way and glints that dance on them: puts a still river, lake or sea in motion (mask it to the water).",
        &[
            ParamDef::float("strength", "Strength", 0.0, 2.0, 1.0).unit(Unit::Times),
            ParamDef::float("ripple", "Ripple", 0.0, 5.0, 0.35).unit(Unit::PercentHeight),
            ParamDef::float("scale", "Ripple size", 0.5, 50.0, 4.0).unit(Unit::PercentHeight),
            ParamDef::float("direction", "Flow direction", -180.0, 180.0, 0.0).unit(Unit::Degrees),
            ParamDef::float("speed", "Flow speed", 0.0, 50.0, 3.0).unit(Unit::PercentHeight),
            ParamDef::float("shimmer", "Glints", 0.0, 2.0, 0.4),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        water,
    )
    .animated()
    .identity("strength", 0.0)
    .wgsl(crate::shaders::WATER),
    EffectDef::new(
        "camera_shake",
        "Camera shake",
        Category::Distort,
        "Hand-held or impact shake, zoomed in just enough to hide the edges.",
        &[
            ParamDef::float("amount", "Amount", 0.0, 20.0, 1.5).unit(Unit::PercentHeight),
            ParamDef::float("frequency", "Frequency", 0.1, 30.0, 6.0).unit(Unit::Hertz),
            ParamDef::float("rotation", "Rotation", 0.0, 20.0, 1.0).unit(Unit::Degrees),
            ParamDef::float("roughness", "Roughness", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::bool("auto_zoom", "Hide edges", true),
            ParamDef::int("seed", "Variation", 0, 999, 0),
        ],
        camera_shake,
    )
    .animated()
    .identity("amount", 0.0)
    .wgsl(crate::shaders::CAMERA_SHAKE),
];

fn wave(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amp = pct_h(img, p.f("amplitude"));
    if amp <= 0.0 {
        return img.clone();
    }
    let len = pct_h(img, p.f("wavelength")).max(1.0);
    let (s, c) = p.f("angle").to_radians().sin_cos();
    let phase = p.f("phase").to_radians() - TAU * p.f("speed") * ctx.time;
    warp(img, Edge::Clamp, |x, y| {
        // The wave travels along `angle` and pushes pixels across it.
        let along = x * c + y * s;
        let d = amp * (TAU * along / len + phase).sin();
        (x - s * d, y + c * d)
    })
}

fn center_px(img: &Image, p: &ParamSet) -> (f32, f32) {
    let c = p.point("center");
    (c[0] * img.width as f32, c[1] * img.height as f32)
}

fn ripple(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amp = pct_h(img, p.f("amplitude"));
    if amp <= 0.0 {
        return img.clone();
    }
    let len = pct_h(img, p.f("wavelength")).max(1.0);
    let (cx, cy) = center_px(img, p);
    let decay = p.f("decay");
    let phase = TAU * p.f("speed") * ctx.time;
    let h = img.height as f32;
    warp(img, Edge::Clamp, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt();
        if r < 1e-3 {
            return (x, y);
        }
        let d = amp * (TAU * r / len - phase).sin() * (-decay * r / h).exp();
        (x + dx / r * d, y + dy / r * d)
    })
}

fn twirl(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let angle = p.f("angle").to_radians();
    let radius = pct_h(img, p.f("radius")).max(1e-3);
    let (cx, cy) = center_px(img, p);
    warp(img, Edge::Clamp, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt();
        if r >= radius {
            return (x, y);
        }
        let k = 1.0 - r / radius;
        let (s, c) = (angle * k * k).sin_cos();
        (cx + dx * c - dy * s, cy + dx * s + dy * c)
    })
}

fn bulge(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let e = 2f32.powf(p.f("amount"));
    let radius = pct_h(img, p.f("radius")).max(1e-3);
    let (cx, cy) = center_px(img, p);
    warp(img, Edge::Clamp, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt();
        if r >= radius || r < 1e-6 {
            return (x, y);
        }
        // Exponent above one samples nearer the centre: magnification.
        let t = r / radius;
        let k = t.powf(e) / t;
        (cx + dx * k, cy + dy * k)
    })
}

fn fisheye(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = p.f("amount");
    let zoom = p.f("zoom");
    if amount == 0.0 && zoom == 1.0 {
        return img.clone();
    }
    let (w, h) = (img.width as f32, img.height as f32);
    let half_diag = 0.5 * (w * w + h * h).sqrt();
    let theta = amount.abs() * 1.2;
    warp(img, Edge::Clamp, |x, y| {
        let (dx, dy) = ((x - w * 0.5) / half_diag, (y - h * 0.5) / half_diag);
        let r = (dx * dx + dy * dy).sqrt();
        if r < 1e-6 || theta < 1e-4 {
            return (w * 0.5 + dx * half_diag / zoom, h * 0.5 + dy * half_diag / zoom);
        }
        // tan mapping magnifies the centre and squeezes the edges; its
        // inverse (atan) undoes a lens that did that.
        let rs = if amount > 0.0 { (r * theta).tan() / theta.tan() } else { (r * theta.tan()).atan() / theta };
        let k = rs / r / zoom;
        (w * 0.5 + dx * k * half_diag, h * 0.5 + dy * k * half_diag)
    })
}

fn glass(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = pct_h(img, p.f("distortion"));
    if amount <= 0.0 {
        return img.clone();
    }
    let scale = pct_h(img, p.f("scale")).max(1.0);
    let mode = p.choice("mode");
    warp(img, Edge::Clamp, |x, y| {
        let (u, v) = (x / scale, y / scale);
        match mode {
            0 => {
                // Frosted: fine noise, so a soft scatter.
                let n1 = value_noise(u * 4.0, v * 4.0, 11) * 0.7 + hash2(x as i32, y as i32, 5) * 0.3;
                let n2 = value_noise(u * 4.0 + 31.0, v * 4.0 + 17.0, 13) * 0.7 + hash2(x as i32, y as i32, 9) * 0.3;
                (x + (n1 - 0.5) * 2.0 * amount, y + (n2 - 0.5) * 2.0 * amount)
            }
            1 => {
                // Reeded: each vertical rib is a little cylinder lens that
                // flips and compresses what is behind it.
                let f = u - u.floor() - 0.5;
                (x - f * 2.0 * amount, y)
            }
            _ => {
                let (fu, fv) = (u - u.floor() - 0.5, v - v.floor() - 0.5);
                let bump = 1.0 - smoothstep(0.3, 0.5, fu.abs().max(fv.abs()));
                (x - fu * 2.0 * amount * (0.5 + bump), y - fv * 2.0 * amount * (0.5 + bump))
            }
        }
    })
}

/// Flowing ripples: two layers of stretched value noise sliding along the
/// flow (one slower and a little across it) push pixels about, and a finer
/// faster layer lights glints. `wgsl/water.wgsl` is the same sum.
fn water(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let k = p.f("strength");
    let amt = pct_h(img, p.f("ripple")) * k;
    let shimmer = p.f("shimmer") * k;
    if amt <= 0.0 && shimmer <= 0.0 {
        return img.clone();
    }
    let scale = pct_h(img, p.f("scale")).max(1.0);
    let (s, c) = p.f("direction").to_radians().sin_cos();
    let v = pct_h(img, p.f("speed")) * ctx.time;
    let seed = ctx.seed ^ (p.i("seed") as u32).wrapping_mul(0x9e37_79b9);
    Image::from_fn(img.width, img.height, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let (u1, v1) = ((fx - c * v) / scale, (fy - s * v) / scale * 2.0);
        let (u2, v2) = ((fx - c * v * 0.6 + s * v * 0.25) / (scale * 0.7), (fy - s * v * 0.6 - c * v * 0.25) / (scale * 0.7) * 2.0);
        let dx = amt * (value_noise(u1, v1, seed) + value_noise(u2, v2, seed.wrapping_add(1)) - 1.0);
        let dy = amt * 0.5 * (value_noise(u1 + 5.2, v1 + 1.3, seed.wrapping_add(2)) + value_noise(u2 + 3.1, v2 + 7.7, seed.wrapping_add(3)) - 1.0);
        let src = img.sample(fx + dx, fy + dy, Edge::Clamp);
        let n = value_noise((fx - c * v * 1.3) / (scale * 0.25), (fy - s * v * 1.3) / (scale * 0.12), seed.wrapping_add(4));
        let g = smoothstep(0.78, 0.97, n) * shimmer * (0.25 + crate::image::luma(src).max(0.0)) * src[3];
        [src[0] + g, src[1] + g, src[2] + g, src[3]]
    })
}

fn noise_displace(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = pct_h(img, p.f("amount"));
    if amount <= 0.0 {
        return img.clone();
    }
    let scale = pct_h(img, p.f("scale")).max(1.0);
    let t = ctx.time * p.f("speed");
    let oct = p.i("octaves") as u32;
    let seed = ctx.seed ^ (p.i("seed") as u32).wrapping_mul(0x9e37_79b9);
    warp(img, Edge::Clamp, |x, y| {
        let (u, v) = (x / scale, y / scale);
        let dx = fbm(u, v, t, oct, seed) - 0.5;
        let dy = fbm(u + 19.7, v + 7.3, t, oct, seed ^ 0x5bd1) - 0.5;
        (x + dx * 2.0 * amount, y + dy * 2.0 * amount)
    })
}

/// Offset (pixels) and rotation (radians) of the shaking camera at `t`.
pub(crate) fn shake_pose(t: f32, freq: f32, amount: f32, rot: f32, rough: f32, seed: u32) -> (f32, f32, f32) {
    let n = |k: u32| {
        let s = seed.wrapping_add(k.wrapping_mul(0x632b_e5ab));
        (noise1(t * freq, s) + rough * 0.5 * noise1(t * freq * 2.7 + 13.0, s ^ 0xffff)) / (1.0 + rough * 0.5)
    };
    (n(1) * amount, n(2) * amount, n(3) * rot)
}

fn camera_shake(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let amount = pct_h(img, p.f("amount"));
    if amount <= 0.0 {
        return img.clone();
    }
    let rot = p.f("rotation").to_radians();
    let seed = ctx.seed ^ (p.i("seed") as u32).wrapping_mul(0x85eb_ca6b);
    let (ox, oy, a) = shake_pose(ctx.time, p.f("frequency"), amount, rot, p.f("roughness"), seed);
    let (w, h) = (img.width as f32, img.height as f32);
    let zoom = if p.b("auto_zoom") {
        // Enough to cover the worst case of the offset and rotation range.
        let ratio = (w / h).max(h / w);
        rot.cos() + rot.sin() * ratio + 2.0 * amount / w.min(h)
    } else {
        1.0
    };
    let (s, c) = (-a).sin_cos();
    warp(img, Edge::Mirror, |x, y| {
        let (dx, dy) = ((x - w * 0.5 - ox) / zoom, (y - h * 0.5 - oy) / zoom);
        (w * 0.5 + dx * c - dy * s, h * 0.5 + dx * s + dy * c)
    })
}
