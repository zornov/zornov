// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Time: echo trails, strobe, frame blend, posterise time. These read
//! earlier frames from [`EffectContext::history`].

use super::{Category, EffectContext, EffectDef};
use crate::image::{lerp_px, over, scale_px, Image};
use crate::ops::mix;
use crate::params::{ParamDef, ParamSet, Unit};

pub static EFFECTS: &[EffectDef] = &[
    EffectDef::new(
        "echo",
        "Echo",
        Category::Time,
        "Fading copies of earlier frames trail behind movement.",
        &[
            ParamDef::int("echoes", "Echoes", 1, 16, 4),
            ParamDef::int("spacing", "Frames apart", 1, 10, 2),
            ParamDef::float("decay", "Decay", 0.0, 1.0, 0.6).unit(Unit::Fraction),
            ParamDef::choice("mode", "Blend", &["Behind", "Light trails", "Lighten"], 0),
            ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
        ],
        echo,
    )
    .identity("amount", 0.0)
    .history(|p, _| (p.i("echoes") * p.i("spacing")) as usize),
    EffectDef::new(
        "strobe",
        "Strobe",
        Category::Time,
        "Rhythmic flashes, blackouts or inversions.",
        &[
            ParamDef::float("rate", "Rate", 0.5, 30.0, 4.0).unit(Unit::Hertz),
            ParamDef::float("duty", "Picture time", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::choice("mode", "Flash", &["Colour", "Transparent", "Invert"], 0),
            ParamDef::colour("colour", "Colour", [1.0, 1.0, 1.0, 1.0]),
            ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
        ],
        strobe,
    )
    .animated()
    .identity("amount", 0.0),
    EffectDef::new(
        "frame_blend",
        "Frame blend",
        Category::Time,
        "Averages neighbouring frames for dreamy motion smear.",
        &[
            ParamDef::int("frames", "Frames", 2, 16, 3),
            ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
        ],
        frame_blend,
    )
    .identity("amount", 0.0)
    .history(|p, _| (p.i("frames") - 1) as usize),
    EffectDef::new(
        "posterise_time",
        "Posterise time",
        Category::Time,
        "Holds frames for a choppy, low-frame-rate look.",
        &[ParamDef::float("fps", "Frame rate", 1.0, 60.0, 8.0).unit(Unit::Hertz)],
        posterise_time,
    )
    .animated()
    .identity("fps", 60.0)
    .history(|p, rate| ((rate / p.f("fps")).ceil() as usize).saturating_sub(1)),
];

fn echo(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let spacing = p.i("spacing") as usize;
    let decay = p.f("decay");
    let mode = p.choice("mode");
    let echoes: Vec<(&Image, f32)> = (1..=p.i("echoes") as usize)
        .filter_map(|k| ctx.history.get(k * spacing - 1).map(|f| (f, decay.powi(k as i32))))
        .filter(|(f, _)| f.width == img.width && f.height == img.height)
        .collect();
    if echoes.is_empty() {
        return img.clone();
    }
    let out = img.map_xy(|x, y, cur| match mode {
        0 => {
            // Oldest at the back, each newer one over it, the live frame on top.
            let mut acc = [0.0f32; 4];
            for &(f, w) in echoes.iter().rev() {
                acc = over(scale_px(f.get(x, y), w), acc);
            }
            over(cur, acc)
        }
        1 => {
            let mut acc = cur;
            for &(f, w) in &echoes {
                let e = f.get(x, y);
                for c in 0..3 {
                    acc[c] += e[c] * w;
                }
                acc[3] = acc[3].max(e[3] * w);
            }
            acc
        }
        _ => {
            let mut acc = cur;
            for &(f, w) in &echoes {
                let e = scale_px(f.get(x, y), w);
                for c in 0..4 {
                    acc[c] = acc[c].max(e[c]);
                }
            }
            acc
        }
    });
    mix(img, &out, p.f("amount"))
}

fn strobe(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let phase = (ctx.time * p.f("rate")).rem_euclid(1.0);
    if phase < p.f("duty") {
        return img.clone();
    }
    let c = p.colour("colour");
    let amount = p.f("amount");
    img.map(|px| {
        let flash = match p.choice("mode") {
            0 => [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]],
            1 => [0.0; 4],
            _ => [
                (px[3] - px[0]).max(0.0),
                (px[3] - px[1]).max(0.0),
                (px[3] - px[2]).max(0.0),
                px[3],
            ],
        };
        lerp_px(px, flash, amount)
    })
}

fn frame_blend(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let frames: Vec<&Image> = std::iter::once(img)
        .chain(ctx.history.iter().take(p.i("frames") as usize - 1))
        .filter(|f| f.width == img.width && f.height == img.height)
        .collect();
    let n = frames.len() as f32;
    let avg = img.map_xy(|x, y, _| {
        let mut acc = [0.0f32; 4];
        for f in &frames {
            let v = f.get(x, y);
            for c in 0..4 {
                acc[c] += v[c] / n;
            }
        }
        acc
    });
    mix(img, &avg, p.f("amount"))
}

fn posterise_time(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let fps = p.f("fps");
    let rate = ctx.frame_rate.max(1.0);
    if fps >= rate {
        return img.clone();
    }
    // Frames since the last "tick" of the slower clock.
    let n = (ctx.time * rate).round() as i64;
    let tick = (ctx.time * fps + 1e-4).floor() as i64;
    let tick_frame = ((tick as f64) * rate as f64 / fps as f64 - 1e-4).ceil() as i64;
    let back = (n - tick_frame).max(0) as usize;
    if back == 0 || ctx.history.is_empty() {
        return img.clone();
    }
    let f = &ctx.history[(back - 1).min(ctx.history.len() - 1)];
    if f.width == img.width && f.height == img.height { f.clone() } else { img.clone() }
}
