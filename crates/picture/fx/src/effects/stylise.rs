// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stylise: posterise, halftone, comic ink, oil paint, sketch, emboss,
//! pixelate, mosaic tiles, kaleidoscope, mirror, edge detect.

use super::{apply_mix, Category, EffectContext, EffectDef, MIX};
use crate::image::{linear_to_srgb, luma, premultiply, srgb_to_linear, unpremultiply, Edge, Image, Px};
use crate::math::{clamp01, hash2, smoothstep, TAU};
use crate::ops::{gaussian_blur, pct_h, warp};
use crate::params::{ParamDef, ParamSet, Unit};

pub static EFFECTS: &[EffectDef] = &[
    EffectDef::new(
        "posterise",
        "Posterise",
        Category::Stylise,
        "Reduces each colour channel to a few flat steps.",
        &[ParamDef::int("levels", "Levels", 2, 32, 6).animated(), MIX],
        posterise,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::POSTERISE),
    EffectDef::new(
        "halftone",
        "Halftone",
        Category::Stylise,
        "Printed-dot look, in black ink or three colour inks.",
        &[
            ParamDef::float("cell", "Dot size", 0.2, 10.0, 1.2).unit(Unit::PercentHeight),
            ParamDef::float("angle", "Screen angle", -90.0, 90.0, 45.0).unit(Unit::Degrees),
            ParamDef::choice("mode", "Inks", &["Black", "Colour"], 1),
            ParamDef::colour("paper", "Paper", [0.94, 0.92, 0.86, 1.0]),
            MIX,
        ],
        halftone,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::HALFTONE),
    EffectDef::new(
        "comic_ink",
        "Comic ink",
        Category::Stylise,
        "Flat colour bands with bold ink outlines.",
        &[
            ParamDef::int("levels", "Shading levels", 2, 8, 4),
            ParamDef::float("edges", "Outline strength", 0.0, 2.0, 1.0),
            ParamDef::float("threshold", "Outline threshold", 0.0, 1.0, 0.15).unit(Unit::Fraction),
            ParamDef::float("width", "Outline width", 0.0, 2.0, 0.15).unit(Unit::PercentHeight),
            ParamDef::float("saturation", "Saturation boost", 0.0, 1.0, 0.3).unit(Unit::Fraction),
            MIX,
        ],
        comic_ink,
    )
    .identity("mix", 0.0),
    EffectDef::new(
        "oil_paint",
        "Oil paint",
        Category::Stylise,
        "Painterly smoothing that keeps edges crisp.",
        &[ParamDef::float("radius", "Brush size", 0.0, 3.0, 0.6).unit(Unit::PercentHeight), MIX],
        oil_paint,
    )
    .identity("radius", 0.0),
    EffectDef::new(
        "sketch",
        "Sketch",
        Category::Stylise,
        "Pencil drawing on paper.",
        &[
            ParamDef::float("detail", "Stroke size", 0.0, 3.0, 0.8).unit(Unit::PercentHeight),
            ParamDef::float("darkness", "Darkness", 0.0, 3.0, 1.5),
            ParamDef::float("shading", "Shading", 0.0, 1.0, 0.35).unit(Unit::Fraction),
            ParamDef::float("colour", "Colour", 0.0, 1.0, 0.0).unit(Unit::Fraction),
            ParamDef::colour("paper", "Paper", [0.95, 0.93, 0.88, 1.0]),
            MIX,
        ],
        sketch,
    )
    .identity("mix", 0.0),
    EffectDef::new(
        "emboss",
        "Emboss",
        Category::Stylise,
        "Raised relief lit from one side.",
        &[
            ParamDef::float("angle", "Light angle", 0.0, 360.0, 135.0).unit(Unit::Degrees),
            ParamDef::float("depth", "Depth", 0.0, 10.0, 2.0),
            ParamDef::bool("colour", "Keep colour", false),
            MIX,
        ],
        emboss,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::EMBOSS),
    EffectDef::new(
        "pixelate",
        "Pixelate",
        Category::Stylise,
        "Large square pixels.",
        &[ParamDef::float("size", "Pixel size", 0.0, 20.0, 2.0).unit(Unit::PercentHeight)],
        pixelate,
    )
    .identity("size", 0.0)
    .wgsl(crate::shaders::PIXELATE),
    EffectDef::new(
        "mosaic",
        "Mosaic tiles",
        Category::Stylise,
        "Glazed square tiles set in grout.",
        &[
            ParamDef::float("size", "Tile size", 0.5, 20.0, 3.0).unit(Unit::PercentHeight),
            ParamDef::float("grout", "Grout width", 0.0, 0.5, 0.08).unit(Unit::Fraction),
            ParamDef::colour("grout_colour", "Grout colour", [0.12, 0.11, 0.1, 1.0]),
            ParamDef::float("variation", "Tile variation", 0.0, 1.0, 0.25).unit(Unit::Fraction),
            ParamDef::float("bevel", "Bevel", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            MIX,
        ],
        mosaic,
    )
    .identity("mix", 0.0),
    EffectDef::new(
        "kaleidoscope",
        "Kaleidoscope",
        Category::Stylise,
        "Mirrored wedges arranged around a centre.",
        &[
            ParamDef::int("segments", "Segments", 2, 32, 6),
            ParamDef::float("angle", "Rotation", -360.0, 360.0, 0.0).unit(Unit::Degrees),
            ParamDef::point("center", "Centre", [0.5, 0.5]),
            ParamDef::float("zoom", "Zoom", 0.25, 4.0, 1.0).unit(Unit::Times),
            MIX,
        ],
        kaleidoscope,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::KALEIDOSCOPE),
    EffectDef::new(
        "mirror",
        "Mirror",
        Category::Stylise,
        "Reflects one half of the picture onto the other.",
        &[
            ParamDef::choice(
                "mode",
                "Mirror",
                &["Left onto right", "Right onto left", "Top onto bottom", "Bottom onto top", "Four ways"],
                0,
            ),
            ParamDef::float("position", "Mirror line", 0.0, 1.0, 0.5).unit(Unit::Fraction),
            MIX,
        ],
        mirror,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::MIRROR),
    EffectDef::new(
        "edge_detect",
        "Edge detect",
        Category::Stylise,
        "Shows only the outlines in the picture.",
        &[
            ParamDef::choice("mode", "Style", &["Light on dark", "Colour edges", "Dark on light"], 0),
            ParamDef::float("strength", "Strength", 0.0, 4.0, 1.0),
            MIX,
        ],
        edge_detect,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::EDGE_DETECT),
];

/// Straight colour in sRGB encoding, where equal steps look equal.
fn perceptual(p: Px) -> [f32; 3] {
    unpremultiply(p).map(|c| linear_to_srgb(c.max(0.0)))
}

fn from_perceptual(c: [f32; 3], a: f32) -> Px {
    premultiply(c.map(|v| srgb_to_linear(clamp01(v))), a)
}

fn posterise(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let n = (p.i("levels") - 1).max(1) as f32;
    let out = img.map(|px| {
        let c = perceptual(px).map(|v| (clamp01(v) * n + 0.5).floor() / n);
        from_perceptual(c, px[3])
    });
    apply_mix(img, out, p)
}

/// Coverage of a round dot of relative area `ink` at cell-local position
/// `(fx, fy)` (each -0.5..0.5), antialiased over `aa` cell units.
fn dot(ink: f32, fx: f32, fy: f32, aa: f32) -> f32 {
    let r = (clamp01(ink) / std::f32::consts::PI).sqrt();
    let d = (fx * fx + fy * fy).sqrt();
    smoothstep(r + aa, r - aa, d)
}

fn cell_coords(x: f32, y: f32, cell: f32, angle: f32) -> (f32, f32, f32, f32) {
    let (s, c) = angle.sin_cos();
    let (u, v) = ((x * c + y * s) / cell, (-x * s + y * c) / cell);
    let (cu, cv) = (u.floor() + 0.5, v.floor() + 0.5);
    // Back to pixels for the centre of this cell, where the ink is measured.
    let (px, py) = ((cu * c - cv * s) * cell, (cu * s + cv * c) * cell);
    (u - cu, v - cv, px, py)
}

fn halftone(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let cell = pct_h(img, p.f("cell")).max(1.0);
    let angle = p.f("angle").to_radians();
    let colour = p.choice("mode") == 1;
    let paper = p.colour("paper");
    let aa = 0.7 / cell;
    let out = img.map_xy(|x, y, px| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let rgb = if colour {
            // Cyan, magenta and yellow screens at different angles, as in
            // print, multiplied onto the paper.
            let mut rgb = [paper[0], paper[1], paper[2]];
            for (ch, offset) in [(0usize, 15.0f32), (1, 75.0), (2, 0.0)] {
                let (u, v, cx, cy) = cell_coords(fx, fy, cell, angle + offset.to_radians());
                let s = perceptual(img.sample(cx, cy, Edge::Clamp));
                let cov = dot(1.0 - s[ch], u, v, aa);
                rgb[ch] *= 1.0 - cov * 0.95;
            }
            rgb
        } else {
            let (u, v, cx, cy) = cell_coords(fx, fy, cell, angle);
            let s = img.sample(cx, cy, Edge::Clamp);
            let l = linear_to_srgb(luma(unpremultiply_px(s)).max(0.0));
            let cov = dot(1.0 - l, u, v, aa);
            [paper[0], paper[1], paper[2]].map(|c| c * (1.0 - cov * 0.97))
        };
        premultiply(rgb, px[3])
    });
    apply_mix(img, out, p)
}

fn unpremultiply_px(p: Px) -> Px {
    let c = unpremultiply(p);
    [c[0], c[1], c[2], 1.0]
}

/// Sobel gradient magnitude of a scalar field read through `f`.
fn sobel(f: impl Fn(isize, isize) -> f32, x: isize, y: isize) -> (f32, f32) {
    let gx = f(x + 1, y - 1) + 2.0 * f(x + 1, y) + f(x + 1, y + 1) - f(x - 1, y - 1) - 2.0 * f(x - 1, y) - f(x - 1, y + 1);
    let gy = f(x - 1, y + 1) + 2.0 * f(x, y + 1) + f(x + 1, y + 1) - f(x - 1, y - 1) - 2.0 * f(x, y - 1) - f(x + 1, y - 1);
    (gx, gy)
}

fn comic_ink(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let width = pct_h(img, p.f("width")).max(0.5);
    let soft = gaussian_blur(img, width, width, Edge::Clamp);
    let pl = |x: isize, y: isize| linear_to_srgb(luma(soft.fetch(x, y, Edge::Clamp)).max(0.0));
    let n = (p.i("levels") - 1).max(1) as f32;
    let threshold = p.f("threshold");
    let edges = p.f("edges");
    let sat = 1.0 + p.f("saturation");
    let out = img.map_xy(|x, y, px| {
        let c = perceptual(soft.get(x, y));
        let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        // Quantise brightness, keep hue: scale the colour to the band value.
        let band = ((l * n + 0.5).floor() / n).max(0.08);
        let k = band / l.max(1e-3);
        let c = c.map(|v| clamp01(band + (v * k - band) * sat));
        let (gx, gy) = sobel(pl, x as isize, y as isize);
        let g = (gx * gx + gy * gy).sqrt() * width.sqrt() * 0.5;
        let ink = clamp01(smoothstep(threshold, threshold + 0.1, g) * edges);
        from_perceptual(c.map(|v| v * (1.0 - ink)), px[3])
    });
    apply_mix(img, out, p)
}

/// Kuwahara filter: for each pixel, take the mean of whichever of the four
/// surrounding quadrants is most uniform. Summed-area tables make the cost
/// independent of the radius.
fn oil_paint(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let r = pct_h(img, p.f("radius")).round() as isize;
    if r < 1 {
        return img.clone();
    }
    let (w, h) = (img.width, img.height);
    let sw = w + 1;
    // Channels: r, g, b, a, luma, luma^2; f64 keeps the sums exact enough.
    let mut sat = vec![[0.0f64; 6]; sw * (h + 1)];
    for y in 0..h {
        let mut row = [0.0f64; 6];
        for x in 0..w {
            let px = img.get(x, y);
            let l = luma(px) as f64;
            let v = [px[0] as f64, px[1] as f64, px[2] as f64, px[3] as f64, l, l * l];
            for c in 0..6 {
                row[c] += v[c];
                sat[(y + 1) * sw + x + 1][c] = sat[y * sw + x + 1][c] + row[c];
            }
        }
    }
    let region = |x0: isize, y0: isize, x1: isize, y1: isize| {
        let cx = |v: isize| v.clamp(0, w as isize) as usize;
        let cy = |v: isize| v.clamp(0, h as isize) as usize;
        let (x0, x1, y0, y1) = (cx(x0), cx(x1), cy(y0), cy(y1));
        let n = ((x1 - x0) * (y1 - y0)).max(1) as f64;
        let mut s = [0.0f64; 6];
        for c in 0..6 {
            s[c] = (sat[y1 * sw + x1][c] - sat[y0 * sw + x1][c] - sat[y1 * sw + x0][c] + sat[y0 * sw + x0][c]) / n;
        }
        s
    };
    let out = Image::from_fn(w, h, |x, y| {
        let (x, y) = (x as isize, y as isize);
        let quads = [
            region(x - r, y - r, x + 1, y + 1),
            region(x, y - r, x + r + 1, y + 1),
            region(x - r, y, x + 1, y + r + 1),
            region(x, y, x + r + 1, y + r + 1),
        ];
        let best = quads
            .iter()
            .min_by(|a, b| (a[5] - a[4] * a[4]).partial_cmp(&(b[5] - b[4] * b[4])).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        [best[0] as f32, best[1] as f32, best[2] as f32, best[3] as f32].map(|v| v.max(0.0))
    });
    apply_mix(img, out, p)
}

fn sketch(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let r = pct_h(img, p.f("detail")).max(0.5);
    let gray = img.map(|px| {
        let l = linear_to_srgb(luma(unpremultiply_px(px)).max(0.0)).min(1.0);
        [l, l, l, 1.0]
    });
    let inv_blur = gaussian_blur(&gray.map(|c| [1.0 - c[0], 0.0, 0.0, 1.0]), r, r, Edge::Clamp);
    let darkness = p.f("darkness");
    let shading = p.f("shading");
    let colour = p.f("colour");
    let paper = p.colour("paper");
    let out = img.map_xy(|x, y, px| {
        let g = gray.get(x, y)[0];
        let b = inv_blur.get(x, y)[0];
        // Colour dodge of the gray by its blurred negative leaves only the
        // places where brightness changes: pencil lines.
        let dodge = clamp01(g / (1.0 - b).max(1e-3));
        // Lines from the dodge, plus some soft tone so shapes keep volume.
        let pencil = dodge.powf(1.0 + darkness * 2.0) * (1.0 - shading * (1.0 - g));
        let tint = unpremultiply(px);
        let rgb: [f32; 3] = std::array::from_fn(|c| {
            let base = paper[c] + (tint[c].clamp(0.0, 1.0) - paper[c]) * colour;
            base * pencil
        });
        premultiply(rgb, px[3])
    });
    apply_mix(img, out, p)
}

fn emboss(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let (s, c) = p.f("angle").to_radians().sin_cos();
    let depth = p.f("depth");
    let keep = p.b("colour");
    let l = |x: isize, y: isize| linear_to_srgb(luma(img.fetch(x, y, Edge::Clamp)).max(0.0));
    let out = img.map_xy(|x, y, px| {
        let (gx, gy) = sobel(l, x as isize, y as isize);
        // Surface lit from `angle`: slopes facing the light get brighter.
        let shade = clamp01(0.5 + (gx * c - gy * s) * depth * 0.125);
        if keep {
            let k = srgb_to_linear(shade) / srgb_to_linear(0.5);
            [px[0] * k, px[1] * k, px[2] * k, px[3]]
        } else {
            let v = srgb_to_linear(shade);
            [v * px[3], v * px[3], v * px[3], px[3]]
        }
    });
    apply_mix(img, out, p)
}

fn pixelate(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let cell = pct_h(img, p.f("size"));
    if cell < 1.0 {
        return img.clone();
    }
    let (w, h) = (img.width as f32, img.height as f32);
    // Cells are laid out from the frame centre so they stay put when the
    // size is animated.
    let cell_of = |v: f32, n: f32| ((v - n * 0.5) / cell).floor();
    let cols = (w / cell).ceil() as isize + 2;
    let rows = (h / cell).ceil() as isize + 2;
    let c0 = cell_of(0.0, w) as isize;
    let r0 = cell_of(0.0, h) as isize;
    let mut sums = vec![([0.0f32; 4], 0u32); (cols * rows) as usize];
    let idx = |x: usize, y: usize| {
        let cx = cell_of(x as f32 + 0.5, w) as isize - c0;
        let cy = cell_of(y as f32 + 0.5, h) as isize - r0;
        (cy * cols + cx) as usize
    };
    for y in 0..img.height {
        for x in 0..img.width {
            let e = &mut sums[idx(x, y)];
            let px = img.get(x, y);
            for c in 0..4 {
                e.0[c] += px[c];
            }
            e.1 += 1;
        }
    }
    Image::from_fn(img.width, img.height, |x, y| {
        let (s, n) = sums[idx(x, y)];
        s.map(|v| v / n.max(1) as f32)
    })
}

fn mosaic(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let cell = pct_h(img, p.f("size")).max(2.0);
    let grout = p.f("grout") * 0.5;
    let gc = p.colour("grout_colour");
    let variation = p.f("variation");
    let bevel = p.f("bevel");
    let aa = 1.0 / cell;
    let out = img.map_xy(|x, y, px| {
        let (u, v) = ((x as f32 + 0.5) / cell, (y as f32 + 0.5) / cell);
        let (iu, iv) = (u.floor(), v.floor());
        let (fu, fv) = (u - iu - 0.5, v - iv - 0.5);
        // Tile colour: average of five points across the tile.
        let mut col = [0.0f32; 4];
        for (ox, oy) in [(0.5, 0.5), (0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
            let s = img.sample((iu + ox) * cell, (iv + oy) * cell, Edge::Clamp);
            for c in 0..4 {
                col[c] += s[c] * 0.2;
            }
        }
        let jitter = 1.0 + (hash2(iu as i32, iv as i32, 77) - 0.5) * variation * 0.6;
        // Light from the top-left: brighter upper-left rim, darker lower-right.
        let edge = smoothstep(0.5 - grout - 0.12, 0.5 - grout, fu.abs().max(fv.abs()));
        let shade = 1.0 + bevel * edge * (if fu + fv < 0.0 { 0.35 } else { -0.35 });
        let tile = [col[0] * jitter * shade, col[1] * jitter * shade, col[2] * jitter * shade, col[3]];
        let inside = smoothstep(0.5 - grout + aa, 0.5 - grout - aa, fu.abs().max(fv.abs()));
        let g = premultiply([gc[0], gc[1], gc[2]], gc[3] * px[3]);
        crate::image::lerp_px(g, tile.map(|v| v.max(0.0)), inside)
    });
    apply_mix(img, out, p)
}

fn kaleidoscope(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let n = p.i("segments").max(2) as f32;
    let seg = TAU / n;
    let rot = p.f("angle").to_radians();
    let c = p.point("center");
    let (cx, cy) = (c[0] * img.width as f32, c[1] * img.height as f32);
    let zoom = p.f("zoom");
    let out = warp(img, Edge::Mirror, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt() / zoom;
        let a = (dy.atan2(dx) - rot).rem_euclid(seg);
        // Mirror every other wedge so neighbours meet seamlessly.
        let a = if a > seg * 0.5 { seg - a } else { a } + rot;
        (cx + r * a.cos(), cy + r * a.sin())
    });
    apply_mix(img, out, p)
}

fn mirror(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let (w, h) = (img.width as f32, img.height as f32);
    let pos = p.f("position");
    let (mx, my) = (pos * w, pos * h);
    let mode = p.choice("mode");
    let out = warp(img, Edge::Clamp, |x, y| match mode {
        0 => (if x > mx { 2.0 * mx - x } else { x }, y),
        1 => (if x < mx { 2.0 * mx - x } else { x }, y),
        2 => (x, if y > my { 2.0 * my - y } else { y }),
        3 => (x, if y < my { 2.0 * my - y } else { y }),
        _ => (if x > mx { 2.0 * mx - x } else { x }, if y > my { 2.0 * my - y } else { y }),
    });
    apply_mix(img, out, p)
}

fn edge_detect(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let strength = p.f("strength");
    let mode = p.choice("mode");
    let ch = |c: usize| move |x: isize, y: isize| linear_to_srgb(img.fetch(x, y, Edge::Clamp)[c].max(0.0));
    let l = |x: isize, y: isize| linear_to_srgb(luma(img.fetch(x, y, Edge::Clamp)).max(0.0));
    let out = img.map_xy(|x, y, px| {
        let (x, y) = (x as isize, y as isize);
        let mag = |g: (f32, f32)| clamp01((g.0 * g.0 + g.1 * g.1).sqrt() * 0.5 * strength);
        let rgb = match mode {
            1 => [mag(sobel(ch(0), x, y)), mag(sobel(ch(1), x, y)), mag(sobel(ch(2), x, y))],
            2 => [1.0 - mag(sobel(l, x, y)); 3],
            _ => [mag(sobel(l, x, y)); 3],
        };
        premultiply(rgb.map(srgb_to_linear), px[3])
    });
    apply_mix(img, out, p)
}
