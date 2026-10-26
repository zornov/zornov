// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Generators: solid, gradient, noise, grid, checkerboard, countdown, bars
//! and tone, and particles (see [`crate::particles`]). A generator draws over its input (use it on an empty layer to
//! get just the generated picture); `mix` is its opacity.

use super::{Category, EffectContext, EffectDef, MIX};
use crate::image::{linear_to_srgb, lerp_px, over, premultiply, srgb_to_linear, Image, Px};
use crate::math::{fbm, hash3, smoothstep, value_noise3, TAU};
use crate::ops::pct_h;
use crate::params::{ParamDef, ParamSet, Unit};

pub static EFFECTS: &[EffectDef] = &[
    EffectDef::new(
        "solid",
        "Solid colour",
        Category::Generator,
        "A flat colour.",
        &[ParamDef::colour("colour", "Colour", [0.08, 0.09, 0.12, 1.0]), MIX],
        solid,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::SOLID),
    EffectDef::new(
        "gradient",
        "Gradient",
        Category::Generator,
        "Two-colour linear, radial or angle gradient.",
        &[
            ParamDef::choice("kind", "Shape", &["Linear", "Radial", "Angle"], 0),
            ParamDef::colour("colour_a", "Start colour", [0.85, 0.32, 0.18, 1.0]),
            ParamDef::colour("colour_b", "End colour", [0.06, 0.08, 0.35, 1.0]),
            ParamDef::point("start", "Start", [0.5, 0.0]),
            ParamDef::point("end", "End", [0.5, 1.0]),
            ParamDef::bool("perceptual", "Even blend", true),
            MIX,
        ],
        gradient,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::GRADIENT),
    EffectDef::new(
        "noise",
        "Noise",
        Category::Generator,
        "Clouds, turbulence or TV static.",
        &[
            ParamDef::choice("kind", "Kind", &["Clouds", "Turbulence", "Static"], 0),
            ParamDef::float("scale", "Scale", 1.0, 200.0, 30.0).unit(Unit::PercentHeight),
            ParamDef::int("octaves", "Detail", 1, 8, 5),
            ParamDef::float("speed", "Speed", 0.0, 5.0, 0.2),
            ParamDef::float("contrast", "Contrast", 0.0, 4.0, 1.0),
            ParamDef::colour("colour_a", "Dark colour", [0.0, 0.0, 0.0, 1.0]),
            ParamDef::colour("colour_b", "Light colour", [1.0, 1.0, 1.0, 1.0]),
            ParamDef::int("seed", "Variation", 0, 999, 0),
            MIX,
            ParamDef::point("drift", "Drift", [0.0, 0.0]).unit(Unit::PercentHeight),
        ],
        noise,
    )
    .animated()
    .identity("mix", 0.0),
    EffectDef::new(
        "grid",
        "Grid",
        Category::Generator,
        "Evenly spaced lines, for layout guides or graphic backgrounds.",
        &[
            ParamDef::float("cell", "Cell size", 1.0, 50.0, 10.0).unit(Unit::PercentHeight),
            ParamDef::float("width", "Line width", 0.02, 5.0, 0.15).unit(Unit::PercentHeight),
            ParamDef::colour("line", "Line colour", [0.8, 0.8, 0.8, 1.0]),
            ParamDef::colour("background", "Background", [0.0, 0.0, 0.0, 0.0]),
            MIX,
        ],
        grid,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::GRID),
    EffectDef::new(
        "checkerboard",
        "Checkerboard",
        Category::Generator,
        "Alternating squares.",
        &[
            ParamDef::float("size", "Square size", 1.0, 50.0, 6.0).unit(Unit::PercentHeight),
            ParamDef::colour("colour_a", "Colour A", [0.8, 0.8, 0.8, 1.0]),
            ParamDef::colour("colour_b", "Colour B", [0.25, 0.25, 0.25, 1.0]),
            ParamDef::float("angle", "Angle", -180.0, 180.0, 0.0).unit(Unit::Degrees),
            MIX,
        ],
        checkerboard,
    )
    .identity("mix", 0.0)
    .wgsl(crate::shaders::CHECKERBOARD),
    EffectDef::new(
        "countdown",
        "Countdown",
        Category::Generator,
        "Film-leader style countdown with a sweeping hand.",
        &[
            ParamDef::int("start", "Count from", 1, 99, 5),
            ParamDef::float("seconds", "Seconds per number", 0.25, 5.0, 1.0).unit(Unit::Seconds).fixed(),
            ParamDef::colour("ink", "Number colour", [0.95, 0.95, 0.95, 1.0]),
            ParamDef::colour("accent", "Sweep colour", [0.9, 0.42, 0.12, 1.0]),
            ParamDef::colour("background", "Background", [0.05, 0.05, 0.06, 1.0]),
            MIX,
        ],
        countdown,
    )
    .animated()
    .identity("mix", 0.0),
    EffectDef::new(
        "bars",
        "Bars and tone",
        Category::Generator,
        "Colour bars with a grey ramp and black-level patches; pair with `bars_tone` for audio.",
        &[
            ParamDef::choice("level", "Bar level", &["75 %", "100 %"], 0),
            ParamDef::float("tone_hz", "Tone frequency", 100.0, 10000.0, 1000.0).unit(Unit::Hertz).fixed(),
            ParamDef::float("tone_db", "Tone level (dBFS)", -60.0, 0.0, -20.0).fixed(),
            MIX,
        ],
        bars,
    )
    .identity("mix", 0.0),
    EffectDef::new(
        "particles",
        "Particles",
        Category::Generator,
        "Rain, snow, falling leaves, dust motes, fireflies, sparks, smoke, mist, splashes, bubbles, confetti, magic dust, clouds, glints, stars or embers.",
        PARTICLE_PARAMS,
        particles,
    )
    .animated()
    .identity("mix", 0.0)
    .wgsl(crate::shaders::PARTICLES)
    .storage(particles_data),
];

/// The particle generator's settings: a preset, then changes to it.
const PARTICLE_PARAMS: &[ParamDef] = &[
    ParamDef::choice("preset", "Preset", crate::particles::PRESET_NAMES, 0),
    ParamDef::choice("emitter", "Emitter", crate::particles::EMITTER_NAMES, 0),
    ParamDef::point("area_a", "Area from", [0.0, 0.0]),
    ParamDef::point("area_b", "Area to", [1.0, 1.0]),
    ParamDef::float("amount", "Amount", 0.0, 5.0, 1.0).unit(Unit::Times),
    ParamDef::float("size", "Size", 0.1, 5.0, 1.0).unit(Unit::Times),
    ParamDef::float("speed", "Speed", 0.0, 5.0, 1.0).unit(Unit::Times),
    ParamDef::float("direction", "Direction", -180.0, 180.0, 0.0).unit(Unit::Degrees),
    ParamDef::float("gravity", "Gravity", -5.0, 5.0, 1.0).unit(Unit::Times),
    ParamDef::float("wind", "Wind", -100.0, 100.0, 0.0).unit(Unit::PercentHeight),
    ParamDef::float("turbulence", "Turbulence", 0.0, 5.0, 1.0).unit(Unit::Times),
    ParamDef::float("life", "Life", 0.1, 5.0, 1.0).unit(Unit::Times).fixed(),
    ParamDef::colour("colour", "Tint", [1.0, 1.0, 1.0, 1.0]),
    ParamDef::float("opacity", "Opacity", 0.0, 1.0, 1.0).unit(Unit::Fraction),
    ParamDef::float("ground", "Ground line", 0.0, 2.0, 1.0).unit(Unit::Fraction),
    ParamDef::float("horizon", "Horizon", 0.0, 1.0, 0.45).unit(Unit::Fraction),
    ParamDef::choice("on_ground", "On the ground", crate::particles::GROUND_NAMES, 0),
    ParamDef::float("depth", "Depth", 0.0, 2.0, 1.0).unit(Unit::Times),
    ParamDef::choice("look", "Look", crate::particles::LOOK_NAMES, 0),
    ParamDef::int("seed", "Variation", 0, 999, 0),
    MIX,
];

fn particles(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    crate::particles::render(ctx.time, ctx.seed, img, p)
}

fn particles_data(ctx: &EffectContext, w: usize, h: usize, p: &ParamSet) -> Vec<u32> {
    crate::particles::packed(ctx.time, ctx.seed, w, h, p)
}

/// Draws `f(x, y)` (premultiplied) over the input with the `mix` opacity.
fn draw(img: &Image, p: &ParamSet, f: impl Fn(f32, f32) -> Px + Sync) -> Image {
    let m = p.f("mix");
    img.map_xy(|x, y, under| {
        let g = f(x as f32 + 0.5, y as f32 + 0.5);
        over([g[0] * m, g[1] * m, g[2] * m, g[3] * m], under)
    })
}

fn pm(c: [f32; 4]) -> Px {
    premultiply([c[0], c[1], c[2]], c[3])
}

/// Blends two straight colours; `perceptual` mixes in sRGB encoding, which
/// avoids the muddy middle a linear blend of saturated colours shows
/// (colours brighter than 1 are clipped in that mode).
fn blend(a: [f32; 4], b: [f32; 4], t: f32, perceptual: bool) -> Px {
    let t = t.clamp(0.0, 1.0);
    let rgb: [f32; 3] = std::array::from_fn(|c| {
        if perceptual {
            let (ea, eb) = (linear_to_srgb(a[c].min(1.0)), linear_to_srgb(b[c].min(1.0)));
            srgb_to_linear(ea + (eb - ea) * t)
        } else {
            a[c] + (b[c] - a[c]) * t
        }
    });
    premultiply(rgb, a[3] + (b[3] - a[3]) * t)
}

fn solid(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let c = pm(p.colour("colour"));
    draw(img, p, |_, _| c)
}

fn gradient(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let (w, h) = (img.width as f32, img.height as f32);
    let s = p.point("start");
    let e = p.point("end");
    let (sx, sy, ex, ey) = (s[0] * w, s[1] * h, e[0] * w, e[1] * h);
    let (dx, dy) = (ex - sx, ey - sy);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    let base_angle = dy.atan2(dx);
    let kind = p.choice("kind");
    let (a, b) = (p.colour("colour_a"), p.colour("colour_b"));
    let perceptual = p.b("perceptual");
    draw(img, p, |x, y| {
        let (vx, vy) = (x - sx, y - sy);
        let t = match kind {
            0 => (vx * dx + vy * dy) / len2,
            1 => ((vx * vx + vy * vy) / len2).sqrt(),
            _ => (vy.atan2(vx) - base_angle).rem_euclid(TAU) / TAU,
        };
        blend(a, b, t, perceptual)
    })
}

fn noise(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let scale = pct_h(img, p.f("scale")).max(1.0);
    let oct = p.i("octaves") as u32;
    let z = ctx.time * p.f("speed");
    let contrast = p.f("contrast");
    let (a, b) = (p.colour("colour_a"), p.colour("colour_b"));
    let seed = ctx.seed ^ (p.i("seed") as u32).wrapping_mul(0x9e37_79b9);
    let kind = p.choice("kind");
    let frame = (ctx.time * ctx.frame_rate.max(1.0)).round() as i32;
    // Drift slides the pattern (% height per second), so clouds travel.
    let drift = p.point("drift");
    let (ox, oy) = (pct_h(img, drift[0]) * ctx.time, pct_h(img, drift[1]) * ctx.time);
    draw(img, p, |x, y| {
        let (u, v) = ((x - ox) / scale, (y - oy) / scale);
        let n = match kind {
            0 => fbm(u, v, z, oct, seed),
            1 => {
                let mut sum = 0.0;
                let mut amp = 0.5;
                let mut norm = 0.0;
                for o in 0..oct {
                    let f = (1u32 << o) as f32;
                    sum += amp * (value_noise3(u * f, v * f, z * f, seed + o) * 2.0 - 1.0).abs();
                    norm += amp;
                    amp *= 0.5;
                }
                sum / norm
            }
            _ => hash3(x as i32, y as i32, frame, seed),
        };
        let t = (n - 0.5) * contrast + 0.5;
        blend(a, b, t, true)
    })
}

fn grid(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let cell = pct_h(img, p.f("cell")).max(1.0);
    let half = pct_h(img, p.f("width")) * 0.5;
    let (line, bg) = (pm(p.colour("line")), pm(p.colour("background")));
    let (cx, cy) = (img.width as f32 * 0.5, img.height as f32 * 0.5);
    draw(img, p, |x, y| {
        // Lines are centred on the frame so a grid looks symmetric.
        let d = |v: f32| {
            let f = (v / cell).rem_euclid(1.0);
            f.min(1.0 - f) * cell
        };
        let dist = d(x - cx).min(d(y - cy));
        let k = smoothstep(half + 0.5, half - 0.5, dist);
        lerp_px(bg, line, k)
    })
}

fn checkerboard(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let size = pct_h(img, p.f("size")).max(1.0);
    let (a, b) = (pm(p.colour("colour_a")), pm(p.colour("colour_b")));
    let (s, c) = p.f("angle").to_radians().sin_cos();
    let (cx, cy) = (img.width as f32 * 0.5, img.height as f32 * 0.5);
    draw(img, p, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let (u, v) = ((dx * c + dy * s) / size, (-dx * s + dy * c) / size);
        // Antialias by measuring how close we are to a square edge.
        let edge = |t: f32| {
            let f = t.rem_euclid(1.0);
            (f.min(1.0 - f)) * size
        };
        let parity = ((u.floor() + v.floor()) as i64).rem_euclid(2) == 0;
        let soft = smoothstep(0.0, 1.0, edge(u).min(edge(v)));
        let k = if parity { 0.5 + 0.5 * soft } else { 0.5 - 0.5 * soft };
        lerp_px(b, a, k)
    })
}

/// Segments lit for digits 0-9, bits a..g (a = top, then clockwise, g = middle).
const SEGMENTS: [u8; 10] = [0x3f, 0x06, 0x5b, 0x4f, 0x66, 0x6d, 0x7d, 0x07, 0x7f, 0x6f];

/// Coverage of a seven-segment digit in a box 1 wide, 2 tall, at local
/// coordinates `(x, y)` with the origin at its centre. `aa` is the
/// antialiasing width in the same units.
fn digit_coverage(d: u32, x: f32, y: f32, aa: f32) -> f32 {
    let mask = SEGMENTS[(d % 10) as usize];
    let t = 0.11; // half thickness
    // (centre x, centre y, horizontal?)
    let segs = [
        (0.0, -0.9, true),
        (0.45, -0.45, false),
        (0.45, 0.45, false),
        (0.0, 0.9, true),
        (-0.45, 0.45, false),
        (-0.45, -0.45, false),
        (0.0, 0.0, true),
    ];
    let mut cov = 0.0f32;
    for (i, &(sx, sy, horiz)) in segs.iter().enumerate() {
        if mask & (1 << i) == 0 {
            continue;
        }
        let (lx, ly) = if horiz { (x - sx, y - sy) } else { (y - sy, x - sx) };
        // Capsule: a segment of half length 0.33 with rounded ends.
        let px = (lx.abs() - 0.33).max(0.0);
        let dist = (px * px + ly * ly).sqrt() - t;
        cov = cov.max(smoothstep(aa, -aa, dist));
    }
    cov
}

fn countdown(ctx: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let secs = p.f("seconds");
    let start = p.i("start");
    let elapsed = ctx.time.max(0.0) / secs;
    let number = (start - elapsed.floor() as i32).max(0) as u32;
    let sweep = elapsed.fract();
    let (ink, accent, bg) = (p.colour("ink"), p.colour("accent"), p.colour("background"));
    let (w, h) = (img.width as f32, img.height as f32);
    let (cx, cy) = (w * 0.5, h * 0.5);
    let aa = 1.0 / h;
    let digits: Vec<u32> = number.to_string().bytes().map(|b| (b - b'0') as u32).collect();
    draw(img, p, |x, y| {
        let (u, v) = ((x - cx) / h, (y - cy) / h);
        let r = (u * u + v * v).sqrt();
        let mut c = pm(bg);
        // Sweep: a wedge clockwise from twelve o'clock.
        let angle = u.atan2(-v).rem_euclid(TAU) / TAU;
        if angle < sweep && number > 0 {
            let k = smoothstep(0.44 + aa, 0.44 - aa, r);
            c = lerp_px(c, pm([accent[0], accent[1], accent[2], accent[3] * 0.55]), k);
        }
        // Two thin rings and a crosshair.
        let ring = |rad: f32, t: f32| smoothstep(t + aa, t - aa, (r - rad).abs());
        let line = |d: f32| smoothstep(0.003 + aa, 0.003 - aa, d.abs());
        let marks = ring(0.44, 0.004).max(ring(0.36, 0.003)).max(line(u)).max(line(v));
        c = lerp_px(c, pm([ink[0], ink[1], ink[2], ink[3] * 0.7]), marks);
        // Number: digits 0.2 frame heights wide, 0.4 tall.
        let n = digits.len() as f32;
        let dw = 0.2;
        let gap = 0.06;
        let total = n * dw + (n - 1.0) * gap;
        let mut cov = 0.0f32;
        for (i, &d) in digits.iter().enumerate() {
            let left = -total * 0.5 + i as f32 * (dw + gap);
            let (lx, ly) = ((u - left - dw * 0.5) / dw, v / (dw));
            cov = cov.max(digit_coverage(d, lx, ly, aa / dw * 1.5));
        }
        lerp_px(c, pm(ink), cov)
    })
}

fn bars(_: &EffectContext, img: &Image, p: &ParamSet) -> Image {
    let level = if p.choice("level") == 0 { 0.75 } else { 1.0 };
    let enc = |r: f32, g: f32, b: f32| [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0];
    let l = level;
    let top = [
        enc(l, l, l),
        enc(l, l, 0.0),
        enc(0.0, l, l),
        enc(0.0, l, 0.0),
        enc(l, 0.0, l),
        enc(l, 0.0, 0.0),
        enc(0.0, 0.0, l),
    ];
    let mid = [
        enc(0.0, 0.0, l),
        enc(0.0, 0.0, 0.0),
        enc(l, 0.0, l),
        enc(0.0, 0.0, 0.0),
        enc(0.0, l, l),
        enc(0.0, 0.0, 0.0),
        enc(l, l, l),
    ];
    let (w, h) = (img.width as f32, img.height as f32);
    draw(img, p, |x, y| {
        let u = x / w;
        let v = y / h;
        let col = ((u * 7.0) as usize).min(6);
        if v < 0.67 {
            top[col]
        } else if v < 0.75 {
            mid[col]
        } else if u < 3.0 / 7.0 {
            // Smooth ramp for checking banding and gamma.
            let t = u / (3.0 / 7.0);
            enc(t, t, t)
        } else {
            // Black-level patches: below black, black, just above, white.
            let k = ((u - 3.0 / 7.0) / (4.0 / 7.0) * 5.0) as usize;
            let v = [0.0, 0.0, 0.02, 0.04, 1.0][k.min(4)];
            enc(v, v, v)
        }
    })
}

/// The tone that goes with the bars: a sine at `tone_hz` and `tone_db`
/// dBFS, starting at sample `start` of the clip.
pub fn bars_tone(p: &ParamSet, sample_rate: u32, start: u64, len: usize) -> Vec<f32> {
    let amp = 10f32.powf(p.f("tone_db") / 20.0);
    let step = p.f("tone_hz") as f64 / sample_rate.max(1) as f64;
    (0..len)
        .map(|i| {
            // Phase in f64 so long clips do not drift.
            let phase = ((start + i as u64) as f64 * step).fract();
            (phase * std::f64::consts::TAU).sin() as f32 * amp
        })
        .collect()
}
