// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keying: chroma key, luma key, difference key. Each makes parts of the
//! picture transparent; AI cut-out lives in the `ai` crate.

use super::{Category, EffectContext, EffectDef};
use crate::image::{linear_to_srgb, lerp_px, unpremultiply, Edge, Image};
use crate::keycolour;
use crate::math::smoothstep;
use crate::ops::{gaussian_blur, pct_h};
use crate::params::{ParamDef, ParamSet, Unit};

const SHOW_MATTE: ParamDef = ParamDef::bool("show_matte", "Show matte", false);
const AMOUNT: ParamDef = ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction);

pub static EFFECTS: &[EffectDef] = &[
    EffectDef::new(
        "chroma_key",
        "Chroma key",
        Category::Keying,
        "Removes a screen of any colour: green, blue, or a plain white, black or grey backdrop. Key colour is the backdrop; Tolerance is how far from it still goes (about 0.1 for a clean white sheet, 0.35 for an uneven screen); Softness the ramp above that; Spill removes the screen colour from edges (and, for a white or black backdrop, the light or dark fringe).",
        &[
            ParamDef::colour("key", "Key colour", [0.05, 0.55, 0.1, 1.0]),
            ParamDef::float("tolerance", "Tolerance", 0.0, 1.0, 0.35).unit(Unit::Fraction),
            ParamDef::float("softness", "Softness", 0.0, 1.0, 0.15).unit(Unit::Fraction),
            ParamDef::float("spill", "Spill suppression", 0.0, 1.0, 0.6).unit(Unit::Fraction),
            ParamDef::float("feather", "Edge feather", 0.0, 2.0, 0.1).unit(Unit::PercentHeight),
            ParamDef::float("choke", "Choke", -2.0, 2.0, 0.0).unit(Unit::PercentHeight),
            SHOW_MATTE,
            AMOUNT,
        ],
        chroma_key,
    )
    .identity("amount", 0.0)
    .wgsl(crate::shaders::CHROMA_KEY),
    EffectDef::new(
        "luma_key",
        "Luma key",
        Category::Keying,
        "Removes dark areas, or with Remove: Brights the bright ones such as a white background. Threshold is how much of that end of the brightness range goes (0.1 removes the top or bottom tenth), Softness the ramp; edges lose the removed colour so no fringe is left.",
        &[
            ParamDef::float("threshold", "Threshold", 0.0, 1.0, 0.1).unit(Unit::Fraction),
            ParamDef::float("softness", "Softness", 0.0, 1.0, 0.1).unit(Unit::Fraction),
            ParamDef::choice("mode", "Remove", &["Darks", "Brights"], 0),
            SHOW_MATTE,
            AMOUNT,
        ],
        luma_key,
    )
    .identity("amount", 0.0)
    .wgsl(crate::shaders::LUMA_KEY),
    EffectDef::new(
        "difference_key",
        "Difference key",
        Category::Keying,
        "Removes everything that matches a clean background plate.",
        &[
            ParamDef::float("threshold", "Threshold", 0.0, 1.0, 0.08).unit(Unit::Fraction),
            ParamDef::float("softness", "Softness", 0.0, 1.0, 0.08).unit(Unit::Fraction),
            ParamDef::float("feather", "Edge feather", 0.0, 2.0, 0.15).unit(Unit::PercentHeight),
            SHOW_MATTE,
            AMOUNT,
        ],
        difference_key,
    )
    .identity("amount", 0.0),
];

/// Perceptual chroma (Cb, Cr per BT.709) of a straight linear colour.
fn chroma(c: [f32; 3]) -> (f32, [f32; 2]) {
    let e = c.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)));
    let y = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
    (y, [(e[2] - y) / 1.8556, (e[0] - y) / 1.5748])
}

/// Min (erode, `r > 0`) or max (dilate, `r < 0`) filter over a square
/// window, done separably on a matte stored in channel 0.
fn choke(m: &mut [f32], w: usize, h: usize, r: isize) {
    if r == 0 {
        return;
    }
    let shrink = r > 0;
    let r = r.unsigned_abs() as isize;
    let pick = |a: f32, b: f32| if shrink { a.min(b) } else { a.max(b) };
    let mut tmp = m.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut v = m[y * w + x];
            for k in -r..=r {
                let xx = (x as isize + k).clamp(0, w as isize - 1) as usize;
                v = pick(v, m[y * w + xx]);
            }
            tmp[y * w + x] = v;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut v = tmp[y * w + x];
            for k in -r..=r {
                let yy = (y as isize + k).clamp(0, h as isize - 1) as usize;
                v = pick(v, tmp[yy * w + x]);
            }
            m[y * w + x] = v;
        }
    }
}

/// Applies a matte (1 keeps the pixel) after optional choke and feather,
/// and returns either the keyed picture or the matte itself.
fn finish(img: &Image, keyed: Image, mut matte: Vec<f32>, choke_px: f32, feather_px: f32, p: &ParamSet) -> Image {
    let (w, h) = (img.width, img.height);
    // Keep the choke cost bounded on huge frames.
    choke(&mut matte, w, h, choke_px.round().clamp(-64.0, 64.0) as isize);
    if feather_px > 0.25 {
        let m = Image { width: w, height: h, pixels: matte.iter().map(|&v| [v, 0.0, 0.0, 0.0]).collect() };
        let b = gaussian_blur(&m, feather_px, feather_px, Edge::Clamp);
        matte = b.pixels.iter().map(|q| q[0]).collect();
    }
    let show = p.b("show_matte");
    let amount = p.f("amount");
    Image::from_fn(w, h, |x, y| {
        let m = matte[y * w + x].clamp(0.0, 1.0);
        let k = keyed.get(x, y);
        let out = if show {
            let v = m * k[3];
            [v, v, v, 1.0]
        } else {
            [k[0] * m, k[1] * m, k[2] * m, k[3] * m]
        };
        lerp_px(img.get(x, y), out, amount)
    })
}

fn chroma_key(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let key = p.colour("key");
    let key3 = [key[0], key[1], key[2]];
    let tol = p.f("tolerance");
    let soft = p.f("softness");
    let spill = p.f("spill");
    // A white, black or grey key weighs the luma difference and takes the
    // key colour back out of soft edges instead of "despilling" a channel.
    let neutral = keycolour::neutrality(key3);
    let unmix_k = neutral * (spill / 0.6).clamp(0.0, 1.0);
    // The channel the screen is strongest in is the one that spills.
    let dom = (0..3).max_by(|&a, &b| key[a].partial_cmp(&key[b]).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(1);
    let mut matte = Vec::with_capacity(img.pixels.len());
    let keyed = img.map(|px| {
        let c = unpremultiply(px);
        let d = keycolour::distance(c, key3);
        let m = smoothstep(tol, tol + soft, d);
        matte.push(m);
        let others = [(dom + 1) % 3, (dom + 2) % 3];
        let limit = 0.5 * (c[others[0]] + c[others[1]]);
        let mut out = c;
        out[dom] -= spill * (1.0 - neutral) * (c[dom] - limit).max(0.0);
        if unmix_k > 0.0 {
            let u = keycolour::unmix(out, key3, m);
            out = std::array::from_fn(|i| out[i] + (u[i] - out[i]) * unmix_k);
        }
        [out[0] * px[3], out[1] * px[3], out[2] * px[3], px[3]]
    });
    finish(img, keyed, matte, pct_h(img, p.f("choke")), pct_h(img, p.f("feather")), p)
}

fn luma_key(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let t = p.f("threshold");
    let s = p.f("softness");
    let brights = p.choice("mode") == 1;
    // The removed end is the colour the edges are un-mixed from.
    let key = if brights { [1.0; 3] } else { [0.0; 3] };
    let mut matte = Vec::with_capacity(img.pixels.len());
    let keyed = img.map(|px| {
        let c = unpremultiply(px);
        let (l, _) = chroma(c);
        let keep_bright = smoothstep(t, t + s, l);
        let m = if brights { 1.0 - smoothstep(1.0 - t - s, 1.0 - t, l) } else { keep_bright };
        matte.push(m);
        let fg = keycolour::unmix(c, key, m);
        [fg[0] * px[3], fg[1] * px[3], fg[2] * px[3], px[3]]
    });
    finish(img, keyed, matte, 0.0, 0.0, p)
}

fn difference_key(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let Some(plate) = ctx.aux.filter(|a| a.width == img.width && a.height == img.height) else {
        return img.clone();
    };
    let t = p.f("threshold");
    let s = p.f("softness");
    let matte = img
        .pixels
        .iter()
        .zip(&plate.pixels)
        .map(|(&a, &b)| {
            let (ea, eb) = (unpremultiply(a), unpremultiply(b));
            let d = (0..3)
                .map(|c| (linear_to_srgb(ea[c].clamp(0.0, 1.0)) - linear_to_srgb(eb[c].clamp(0.0, 1.0))).abs())
                .fold(0.0f32, f32::max);
            smoothstep(t, t + s, d)
        })
        .collect();
    finish(img, img.clone(), matte, 0.0, pct_h(img, p.f("feather")), p)
}
