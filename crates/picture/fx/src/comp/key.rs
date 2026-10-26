// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keying deluxe: a colour-difference keyer with core and edge mattes,
//! clean-plate screen correction and screen subtraction for fine edges
//! (hair, motion blur), spill suppression, edge colour extension and
//! light wrap.

use std::sync::Arc;

use super::geo;
use super::nodes::{BASE, NodeCategory, NodeDef, main_or_blank, slot, straight};
use super::{NodeCtx, Plate};
use crate::image::{Image, premultiply};
use crate::math::smoothstep;
use crate::params::{ParamDef, ParamSet, Unit};

pub static NODES: &[NodeDef] = &[
    NodeDef {
        id: "keyer",
        name: "Keyer",
        category: NodeCategory::Keyer,
        description: "Screen key (green, blue, or Custom for any colour including a plain white, black or grey backdrop) with a solid core matte, a detailed edge matte, screen subtraction, spill suppression and garbage/holdout mattes. Give it a clean plate for uneven screens.",
        inputs: &["in", "clean plate", "garbage", "holdout"],
        params: &[
            ParamDef::choice("screen", "Screen", &["Green", "Blue", "Custom"], 0),
            ParamDef::colour("colour", "Screen colour", [0.05, 0.55, 0.1, 1.0]),
            ParamDef::float("balance", "Screen balance", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::float("gain", "Screen gain", 0.25, 4.0, 1.0),
            ParamDef::float("black_clip", "Clip black", 0.0, 1.0, 0.05).unit(Unit::Fraction),
            ParamDef::float("white_clip", "Clip white", 0.0, 1.0, 0.9).unit(Unit::Fraction),
            ParamDef::float("core_shrink", "Core shrink", 0.0, 100.0, 3.0),
            ParamDef::float("core_soften", "Core soften", 0.0, 100.0, 2.0),
            ParamDef::float("edge_band", "Edge band", 0.0, 400.0, 12.0),
            ParamDef::bool("subtract_screen", "Subtract screen from edges", true),
            ParamDef::choice("despill", "Spill", &["Off", "Average", "Balance", "Limit to max"], 1),
            ParamDef::float("spill", "Spill amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
            ParamDef::float("restore", "Restore brightness", 0.0, 1.0, 0.6).unit(Unit::Fraction),
            ParamDef::float("edge_colour", "Fix edge colour", 0.0, 1.0, 0.0).unit(Unit::Fraction),
            ParamDef::choice("output", "Output", &["Keyed", "Matte", "Core", "Edge", "Status", "Despilled (no key)"], 0),
        ],
        run: keyer,
        ..BASE
    },
    NodeDef {
        id: "despill",
        name: "Despill",
        category: NodeCategory::Keyer,
        description: "Removes green or blue spill from skin, hair and edges, keeping the brightness.",
        params: &[
            ParamDef::choice("screen", "Screen", &["Green", "Blue"], 0),
            ParamDef::choice("method", "Limit by", &["Average", "Balance", "Max"], 0),
            ParamDef::float("balance", "Balance", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
            ParamDef::float("restore", "Restore brightness", 0.0, 1.0, 0.6).unit(Unit::Fraction),
            ParamDef::colour("replace", "Replace with", [0.5, 0.5, 0.5, 0.0]),
        ],
        run: despill,
        ..BASE
    },
    NodeDef {
        id: "light_wrap",
        name: "Light wrap",
        category: NodeCategory::Keyer,
        description: "Lets the background's light wrap around the foreground's edges so a keyed subject sits in the plate.",
        inputs: &["fg", "bg"],
        params: &[
            ParamDef::float("width", "Width", 0.0, 400.0, 15.0),
            ParamDef::float("intensity", "Intensity", 0.0, 4.0, 0.6),
            ParamDef::float("highlights", "Favour highlights", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            ParamDef::choice("output", "Output", &["Comp over bg", "Foreground with wrap", "Wrap only"], 0),
        ],
        run: light_wrap,
        ..BASE
    },
    NodeDef {
        id: "edge_extend",
        name: "Edge extend",
        category: NodeCategory::Keyer,
        description: "Pushes solid foreground colour out into soft edges, killing dark or screen-coloured fringes.",
        params: &[ParamDef::float("size", "Size", 1.0, 200.0, 8.0), ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction)],
        run: edge_extend,
        ..BASE
    },
];

/// The screen value of a straight colour: how much the screen channel
/// exceeds the other two, weighted by `balance`.
#[inline]
fn screen_value(c: [f32; 3], dom: usize, balance: f32) -> f32 {
    let (o1, o2) = ((dom + 1) % 3, (dom + 2) % 3);
    // For green the others are blue then red; balance weighs red.
    let (r_like, b_like) = if dom == 1 { (c[0], c[2]) } else { (c[o1], c[o2]) };
    c[dom] - (balance * r_like + (1.0 - balance) * b_like)
}

/// Spill limit for a straight colour by method (0 average, 1 balance,
/// 2 max).
#[inline]
fn spill_limit(c: [f32; 3], dom: usize, method: usize, balance: f32) -> f32 {
    let (o1, o2) = ((dom + 1) % 3, (dom + 2) % 3);
    let (r_like, b_like) = if dom == 1 { (c[0], c[2]) } else { (c[o1], c[o2]) };
    match method {
        1 => balance * r_like + (1.0 - balance) * b_like,
        2 => r_like.max(b_like),
        _ => 0.5 * (r_like + b_like),
    }
}

/// Removes spill from a straight colour; returns the colour and how much
/// was removed.
#[inline]
pub(crate) fn despill_px(c: [f32; 3], dom: usize, method: usize, balance: f32, amount: f32, restore: f32) -> ([f32; 3], f32) {
    let limit = spill_limit(c, dom, method, balance);
    let spill = (c[dom] - limit).max(0.0) * amount;
    let mut o = c;
    o[dom] -= spill;
    // Put back the brightness the spill carried, as neutral light.
    let lw = [0.2126, 0.7152, 0.0722][dom];
    let back = restore * spill * lw;
    (o.map(|v| v + back), spill)
}

fn keyer(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width, ctx.height);
    let plate = slot(inputs, 1);
    let garbage = slot(inputs, 2).map(|g| g.alpha());
    let holdout = slot(inputs, 3).map(|g| g.alpha());
    let screen = p.choice("screen");
    let key = p.colour("colour");
    let dom = match screen {
        0 => 1,
        1 => 2,
        _ => (0..3).max_by(|&a, &b| key[a].total_cmp(&key[b])).unwrap_or(1),
    };
    let screen_colour = match screen {
        0 => [0.05, 0.55, 0.1],
        1 => [0.05, 0.1, 0.55],
        _ => [key[0], key[1], key[2]],
    };
    let balance = p.f("balance");
    let gain = p.f("gain");
    let (bc, wc) = (p.f("black_clip"), p.f("white_clip").max(p.f("black_clip") + 1e-3));
    let ref_value = screen_value(screen_colour, dom, balance).max(1e-3);
    // A white, black or grey backdrop has no dominant channel to subtract:
    // it is keyed by distance from the backdrop colour (or the clean plate).
    let neutral = screen == 2 && crate::keycolour::neutrality(screen_colour) > 0.5;
    // Raw matte (1 keeps), per pixel against the clean plate where given.
    let raw: Vec<f32> = (0..w * h)
        .map(|i| {
            let (c, _) = straight(src.rgba.pixels[i]);
            if neutral {
                let backdrop = match plate {
                    Some(pl) => straight(pl.rgba.pixels[i]).0,
                    None => screen_colour,
                };
                return (crate::keycolour::distance(c, backdrop) * gain / 0.45).clamp(0.0, 1.0);
            }
            let s_ref = match plate {
                Some(pl) => screen_value(straight(pl.rgba.pixels[i]).0, dom, balance).max(ref_value * 0.2),
                None => ref_value,
            };
            let d = screen_value(c, dom, balance);
            (1.0 - d * gain / s_ref).clamp(0.0, 1.0)
        })
        .collect();
    let raw = if neutral { refine_neutral(&src.rgba, plate.map(|p| &p.rgba), screen_colour, raw, w, h) } else { raw };
    // A refined coverage is already the alpha; the black / white clips are
    // for the colour-difference estimate of a green or blue screen.
    let edge: Vec<f32> = if neutral { raw.iter().map(|&v| if v < 0.03 { 0.0 } else { v.clamp(0.0, 1.0) }).collect() } else { raw.iter().map(|&v| smoothstep(bc, wc, v)).collect() };
    let solid: Vec<f32> = raw.iter().map(|&v| if v >= wc { 1.0 } else { 0.0 }).collect();
    let core = geo::blur_plane(&geo::erode_plane(&solid, w, h, p.f("core_shrink")), w, h, p.f("core_soften"));
    let band_px = p.f("edge_band");
    let band = if band_px > 0.0 {
        let grown = geo::erode_plane(&solid, w, h, -band_px);
        geo::blur_plane(&grown, w, h, band_px * 0.25)
    } else {
        vec![1.0; w * h]
    };
    let mut alpha: Vec<f32> = (0..w * h).map(|i| core[i].max(edge[i] * band[i]).clamp(0.0, 1.0)).collect();
    if let Some(g) = &garbage {
        for (a, g) in alpha.iter_mut().zip(g) {
            *a *= 1.0 - g.clamp(0.0, 1.0);
        }
    }
    if let Some(hd) = &holdout {
        for (a, k) in alpha.iter_mut().zip(hd) {
            *a = a.max(k.clamp(0.0, 1.0));
        }
    }
    // Channel despill means nothing on a neutral backdrop; the screen is
    // subtracted from the soft edges instead.
    let method = if neutral { 0 } else { p.choice("despill") };
    let (amount, restore) = (p.f("spill"), p.f("restore"));
    let subtract = p.b("subtract_screen");
    let output = p.choice("output");
    let rgba = Image::from_fn(w, h, |x, y| {
        let i = y * w + x;
        let a = alpha[i];
        let (c, _) = straight(src.rgba.pixels[i]);
        match output {
            1 => return [a, a, a, 1.0],
            2 => return [core[i], core[i], core[i], 1.0],
            3 => return [edge[i] * band[i], edge[i] * band[i], edge[i] * band[i], 1.0],
            4 => {
                return if a > 0.98 {
                    [1.0, 1.0, 1.0, 1.0]
                } else if a > 0.02 {
                    [0.8, 0.05, 0.05, 1.0]
                } else {
                    [0.0, 0.0, 0.0, 1.0]
                };
            }
            _ => {}
        }
        let despilled = |c: [f32; 3]| if method == 0 { c } else { despill_px(c, dom, method - 1, balance, amount, restore).0 };
        if output == 5 {
            return premultiply(despilled(c), 1.0);
        }
        if a <= 1e-5 {
            return [0.0; 4];
        }
        // Un-mix the screen from partly covered pixels: what remains is
        // the foreground's own contribution.
        let s = match plate {
            Some(pl) => straight(pl.rgba.pixels[i]).0,
            None => screen_colour,
        };
        let fg: [f32; 3] = if subtract { std::array::from_fn(|k| ((c[k] - (1.0 - a) * s[k]) / a).max(0.0)) } else { c };
        premultiply(despilled(fg), a)
    });
    let mut out = src.with_rgba(rgba);
    let fix = p.f("edge_colour");
    if fix > 0.0 && output == 0 {
        out.rgba = extend_edges(&out.rgba, 6.0, fix);
    }
    Ok(out)
}

/// Coverage of the soft edge pixels against a plain backdrop. A pixel's
/// distance from the backdrop says how different it is, not how much of it
/// is subject (a dark subject on black is far from black after the first
/// quarter of coverage), so each uncertain pixel is projected onto the line
/// from the backdrop colour to the nearest solid subject colour: half way
/// along it is half covered. Thin details with no solid colour of their
/// own take the nearest one and, being further out along the line, stay
/// solid.
fn refine_neutral(src: &Image, plate: Option<&Image>, screen: [f32; 3], mut raw: Vec<f32>, w: usize, h: usize) -> Vec<f32> {
    let backdrop = |i: usize| match plate {
        Some(p) => straight(p.pixels[i]).0,
        None => screen,
    };
    let sure: Vec<f32> = raw.iter().map(|&v| if v >= 0.5 { 1.0 } else { 0.0 }).collect();
    let solid = geo::erode_plane(&sure, w, h, 2.0);
    if solid.iter().all(|&v| v < 0.5) {
        return raw;
    }
    let weighted = Image::from_fn(w, h, |x, y| {
        let i = y * w + x;
        let k = if solid[i] >= 0.5 { 1.0 } else { 0.0 };
        let c = straight(src.pixels[i]).0;
        [c[0] * k, c[1] * k, c[2] * k, k]
    });
    let mut levels = Vec::new();
    let mut size = 1.0f32;
    while size <= 48.0 {
        levels.push(crate::ops::gaussian_blur(&weighted, size, size, crate::image::Edge::Clamp));
        size *= 2.0;
    }
    for i in 0..w * h {
        if solid[i] >= 0.5 || raw[i] <= 0.0 {
            continue;
        }
        let (x, y) = (i % w, i / w);
        let Some(v) = levels.iter().map(|l| l.get(x, y)).find(|v| v[3] > 0.02) else { continue };
        let fg = [v[0] / v[3], v[1] / v[3], v[2] / v[3]];
        let (c, k) = (straight(src.pixels[i]).0, backdrop(i));
        let (axis, d): ([f32; 3], [f32; 3]) = (std::array::from_fn(|j| fg[j] - k[j]), std::array::from_fn(|j| c[j] - k[j]));
        let len2: f32 = axis.iter().map(|a| a * a).sum();
        if len2 > 1e-4 {
            let t: f32 = axis.iter().zip(&d).map(|(a, b)| a * b).sum::<f32>() / len2;
            // Never less covered than the distance test said at the low end.
            raw[i] = t.clamp(0.0, 1.0).max(if raw[i] < 0.1 { raw[i] } else { 0.0 });
        }
    }
    raw
}

fn despill(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let dom = if p.choice("screen") == 1 { 2 } else { 1 };
    let (method, balance, amount, restore) = (p.choice("method"), p.f("balance"), p.f("amount"), p.f("restore"));
    let rep = p.colour("replace");
    let rgba = src.rgba.map(|q| {
        let (c, a) = straight(q);
        if a <= 1e-6 {
            return q;
        }
        let (mut o, spill) = despill_px(c, dom, method, balance, amount, restore);
        // Optionally put the removed spill back as another colour (the
        // set's ambient light, say).
        if rep[3] > 0.0 {
            for k in 0..3 {
                o[k] += spill * rep[k] * rep[3];
            }
        }
        premultiply(o, a)
    });
    Ok(src.with_rgba(rgba))
}

fn light_wrap(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let fg = main_or_blank(ctx, inputs);
    let Some(bg) = slot(inputs, 1) else { return Ok(fg) };
    let (w, h) = (ctx.width, ctx.height);
    let width = p.f("width");
    let bgb = crate::ops::gaussian_blur(&bg.rgba, width * 0.5, width * 0.5, crate::image::Edge::Clamp);
    let inv: Vec<f32> = fg.rgba.pixels.iter().map(|q| 1.0 - q[3].clamp(0.0, 1.0)).collect();
    let near_edge = geo::blur_plane(&inv, w, h, width * 0.5);
    let intensity = p.f("intensity");
    let hl = p.f("highlights");
    let output = p.choice("output");
    let rgba = Image::from_fn(w, h, |x, y| {
        let i = y * w + x;
        let f = fg.rgba.pixels[i];
        let b = bgb.pixels[i];
        let l = crate::image::luma(b).max(0.0);
        let bias = 1.0 + hl * (2.0 * l.sqrt() - 1.0);
        let k = near_edge[i] * f[3] * intensity * bias.max(0.0);
        let wrap = [b[0] * k, b[1] * k, b[2] * k];
        match output {
            2 => [wrap[0], wrap[1], wrap[2], 0.0],
            1 => [f[0] + wrap[0], f[1] + wrap[1], f[2] + wrap[2], f[3]],
            _ => {
                let o = [f[0] + wrap[0], f[1] + wrap[1], f[2] + wrap[2], f[3]];
                let under = bg.rgba.pixels[i];
                std::array::from_fn(|c| o[c] + under[c] * (1.0 - f[3]))
            }
        }
    });
    Ok(fg.with_rgba(rgba))
}

fn edge_extend(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let rgba = extend_edges(&src.rgba, p.f("size"), p.f("amount"));
    Ok(src.with_rgba(rgba))
}

/// Replaces the colour of soft-edged pixels with colour spread outwards
/// from the solid interior (push-pull over growing blurs).
pub(crate) fn extend_edges(img: &Image, size: f32, amount: f32) -> Image {
    let (w, h) = (img.width, img.height);
    // Solid pixels only, straight colour weighted by solidity.
    let solid = img.map(|q| {
        let (c, a) = straight(q);
        let k = smoothstep(0.9, 1.0, a);
        [c[0] * k, c[1] * k, c[2] * k, k]
    });
    let mut levels = Vec::new();
    let mut s = 1.0f32;
    while s <= size.max(1.0) * 2.0 {
        levels.push(crate::ops::gaussian_blur(&solid, s, s, crate::image::Edge::Clamp));
        s *= 2.0;
    }
    Image::from_fn(w, h, |x, y| {
        let q = img.get(x, y);
        let (c, a) = straight(q);
        if a >= 0.999 || a <= 1e-5 {
            return q;
        }
        let ext = levels.iter().map(|l| l.get(x, y)).find(|v| v[3] > 0.02).map(|v| [v[0] / v[3], v[1] / v[3], v[2] / v[3]]);
        let Some(e) = ext else { return q };
        let k = amount * (1.0 - a);
        premultiply(std::array::from_fn(|i| c[i] + (e[i] - c[i]) * k), a)
    })
}
