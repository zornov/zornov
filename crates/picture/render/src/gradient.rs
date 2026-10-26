// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Gradients without seams.
//!
//! A straight two-colour ramp mixed in linear light shows three faults:
//! brightness does not change evenly (a dark-to-darker backdrop stays light
//! most of the way, then falls off: a hollow look), the eye sees a crease
//! where the ramp starts and stops (Mach bands: the slope jumps there), and
//! two far-apart hues meet in a muddy grey. Here colours are mixed in
//! OKLab, a space where equal steps look equal, through any number of
//! stops on a smooth curve (zero slope at the ends, no kink at a stop: the
//! "eased gradient" made exact rather than with many CSS stops), with
//! transparency mixed premultiplied so a colour fading out never goes grey.
//! Radial gradients fall off like light, with a long tail and no edge.
//!
//! Smooth ramps over few code values (dark backdrops: 80 levels across
//! 1920 pixels) band into visible stripes once video compression removes
//! the output's light dithering. A fixed, fine grain of about one code
//! value (`grain`, 1 by default, 0 for none), the same every frame, is
//! kept by the encoder and hides the steps.

use crate::frame::Frame;
use crate::scene::{Params, ParamsExt};

type Px = [f32; 4];

/// Linear-light sRGB to OKLab (Björn Ottosson, 2020).
fn to_oklab(c: [f32; 3]) -> [f32; 3] {
    let l = 0.412_221_47 * c[0] + 0.536_332_55 * c[1] + 0.051_445_995 * c[2];
    let m = 0.211_903_5 * c[0] + 0.680_699_5 * c[1] + 0.107_396_96 * c[2];
    let s = 0.088_302_46 * c[0] + 0.281_718_85 * c[1] + 0.629_978_7 * c[2];
    let (l, m, s) = (l.max(0.0).cbrt(), m.max(0.0).cbrt(), s.max(0.0).cbrt());
    [0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s, 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s, 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s]
}

fn from_oklab(c: [f32; 3]) -> [f32; 3] {
    let l = c[0] + 0.396_337_78 * c[1] + 0.215_803_76 * c[2];
    let m = c[0] - 0.105_561_346 * c[1] - 0.063_854_17 * c[2];
    let s = c[0] - 0.089_484_18 * c[1] - 1.291_485_5 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s, -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s, -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s]
}

/// A stop: OKLab colour and its chroma (colourfulness), premultiplied by
/// alpha, and alpha.
#[derive(Clone, Copy)]
struct Stop {
    at: f32,
    lab: [f32; 3],
    chroma: f32,
    a: f32,
}

/// The stops a generator's parameters ask for: `stop0`..`stopN` (with
/// `stopN_at` positions, or evenly spaced), else `start` and `end`.
fn stops(p: &Params) -> Vec<Stop> {
    let mut cols: Vec<(Option<f32>, [f32; 4])> = Vec::new();
    for i in 0..16 {
        let k = format!("stop{i}");
        if p.get(&k).is_none() {
            break;
        }
        let at = p.get(&format!("{k}_at")).and_then(|v| v.as_f32());
        cols.push((at, p.colour(&k, [0.0, 0.0, 0.0, 1.0])));
    }
    if cols.len() < 2 {
        cols = vec![(Some(0.0), p.colour("start", [0.0, 0.0, 0.0, 1.0])), (Some(1.0), p.colour("end", [1.0, 1.0, 1.0, 1.0]))];
    }
    let n = cols.len();
    let mut out: Vec<Stop> = cols
        .iter()
        .enumerate()
        .map(|(i, (at, c))| {
            let lab = to_oklab([c[0], c[1], c[2]]);
            Stop { at: at.unwrap_or(i as f32 / (n - 1) as f32).clamp(0.0, 1.0), lab: [lab[0] * c[3], lab[1] * c[3], lab[2] * c[3]], chroma: lab[1].hypot(lab[2]) * c[3], a: c[3] }
        })
        .collect();
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    out
}

/// The colour at `t` (0..1): a smooth curve through the stops, flat at
/// both ends and at no stop a corner. `linear` gives straight segments.
fn sample(stops: &[Stop], t: f32, linear: bool) -> Px {
    let t = t.clamp(0.0, 1.0);
    let n = stops.len();
    if t <= stops[0].at {
        return finish(stops[0].lab, stops[0].a);
    }
    if t >= stops[n - 1].at {
        return finish(stops[n - 1].lab, stops[n - 1].a);
    }
    let i = stops.partition_point(|s| s.at <= t).clamp(1, n - 1) - 1;
    let (a, b) = (stops[i], stops[i + 1]);
    let h = (b.at - a.at).max(1e-6);
    let u = (t - a.at) / h;
    let chan = |get: &dyn Fn(&Stop) -> f32| -> f32 {
        let (p0, p1) = (get(&a), get(&b));
        if linear {
            return p0 + (p1 - p0) * u;
        }
        // Tangents (per unit t): Catmull-Rom inside, flat at the ends.
        let slope = |j: usize| -> f32 {
            if j == 0 || j + 1 >= n {
                return 0.0;
            }
            (get(&stops[j + 1]) - get(&stops[j - 1])) / (stops[j + 1].at - stops[j - 1].at).max(1e-6)
        };
        let (m0, m1) = (slope(i) * h, slope(i + 1) * h);
        let (u2, u3) = (u * u, u * u * u);
        (2.0 * u3 - 3.0 * u2 + 1.0) * p0 + (u3 - 2.0 * u2 + u) * m0 + (-2.0 * u3 + 3.0 * u2) * p1 + (u3 - u2) * m1
    };
    let mut lab = [chan(&|s| s.lab[0]), chan(&|s| s.lab[1]), chan(&|s| s.lab[2])];
    // Keep it as colourful as the stops it lies between: a straight path
    // in OKLab between far-apart hues passes near grey, so the hue comes
    // from that path and the chroma from the stops (OKLCH-like, without
    // its hue jumps near grey).
    let want = chan(&|s| s.chroma).max(0.0);
    let have = lab[1].hypot(lab[2]);
    if have > 1e-5 {
        let k = (want / have).min(8.0);
        lab[1] *= k;
        lab[2] *= k;
    }
    finish(lab, chan(&|s| s.a))
}

/// Premultiplied-OKLab back to premultiplied linear RGB.
fn finish(lab: [f32; 3], a: f32) -> Px {
    let a = a.clamp(0.0, 1.0);
    if a <= 1e-6 {
        return [0.0; 4];
    }
    let c = from_oklab([lab[0] / a, lab[1] / a, lab[2] / a]);
    [c[0].max(0.0) * a, c[1].max(0.0) * a, c[2].max(0.0) * a, a]
}

/// sRGB encoding of a linear value, and back.
fn encode(v: f32) -> f32 {
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.max(0.0).powf(1.0 / 2.4) - 0.055 }
}
fn decode(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// A fixed triangular grain in -1..1 for pixel (x, y), channel `c`.
fn grain_at(x: u32, y: u32, c: u32) -> f32 {
    let h = |mut v: u32| {
        v ^= v >> 16;
        v = v.wrapping_mul(0x7feb_352d);
        v ^= v >> 15;
        v = v.wrapping_mul(0x846c_a68b);
        v ^ (v >> 16)
    };
    let a = h(x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b) ^ c.wrapping_mul(0xc2b2_ae35));
    let b = h(a ^ 0x27d4_eb2f);
    (a as f32 / u32::MAX as f32) + (b as f32 / u32::MAX as f32) - 1.0
}

/// Adds `amount` code values of grain, in the picture's own encoding so it
/// is as strong in the shadows as in the light.
fn grained(px: [f32; 4], x: u32, y: u32, amount: f32) -> [f32; 4] {
    if amount <= 0.0 || px[3] <= 1e-6 {
        return px;
    }
    let a = px[3];
    let mut out = px;
    for c in 0..3 {
        let v = encode(px[c] / a) + grain_at(x, y, c as u32) * amount / 255.0;
        out[c] = decode(v.clamp(0.0, 1.0)) * a;
    }
    out
}

/// Draws a gradient generator: `angle` (degrees, linear), or `radial`
/// with `centre` (fractions of the frame from its middle) and `radius` (a
/// fraction of the shorter side); `ease` "linear" for straight segments.
pub fn draw(p: &Params, width: u32, height: u32) -> Frame {
    let st = stops(p);
    let linear = p.text("ease", "smooth") == "linear";
    let grain = p.f("grain", 1.0).clamp(0.0, 8.0);
    let (w, h) = (width as f32, height as f32);
    if p.flag("radial", false) {
        let centre = p.point("centre", [0.0, 0.0]);
        let (cx, cy) = (w * (0.5 + centre[0]), h * (0.5 + centre[1]));
        let r = (p.f("radius", 0.5) * w.min(h)).max(1.0);
        // Light falls off with a long, soft tail: about 92% of the way
        // to the outer colour at `radius`, all the way only far beyond,
        // so there is no edge anywhere.
        return Frame::from_fn(width, height, |x, y| {
            let d2 = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)) / (r * r);
            grained(sample(&st, 1.0 - (-2.5 * d2).exp(), linear), x, y, grain)
        });
    }
    let (s, c) = p.f("angle", 0.0).to_radians().sin_cos();
    let extent = (w * c).abs() + (h * s).abs();
    Frame::from_fn(width, height, |x, y| {
        let dx = x as f32 + 0.5 - w * 0.5;
        let dy = y as f32 + 0.5 - h * 0.5;
        grained(sample(&st, (dx * c + dy * s) / extent.max(1e-6) + 0.5, linear), x, y, grain)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Param;

    fn params(pairs: &[(&str, Param)]) -> Params {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn srgb(v: f32) -> f32 {
        if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    }

    #[test]
    fn oklab_round_trips() {
        for c in [[0.2, 0.5, 0.9], [1.0, 0.0, 0.0], [0.01, 0.02, 0.05], [0.9, 0.9, 0.9]] {
            let back = from_oklab(to_oklab(c));
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-4, "{c:?} → {back:?}");
            }
        }
    }

    /// A dark backdrop changes brightness evenly (no plateau then cliff),
    /// with no crease: the step between neighbouring columns never jumps.
    #[test]
    fn dark_gradients_are_even_and_seamless() {
        // #2a1b5c → #05060c, as the AI uses for backdrops (linear light).
        let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        let a = [lin(0x2a as f32 / 255.0), lin(0x1b as f32 / 255.0), lin(0x5c as f32 / 255.0), 1.0];
        let b = [lin(0x05 as f32 / 255.0), lin(0x06 as f32 / 255.0), lin(0x0c as f32 / 255.0), 1.0];
        let p = params(&[("start", Param::Colour(a.map(|v| v as f64))), ("end", Param::Colour(b.map(|v| v as f64))), ("grain", Param::Float(0.0))]);
        let f = draw(&p, 400, 4);
        let row: Vec<f32> = (0..400).map(|x| srgb(f.pixels[x][2]) * 255.0).collect();
        // Perceived brightness at a quarter, half, three quarters: close to
        // even steps (the old linear-light mix had 70% of the change in
        // the last fifth).
        let (q1, mid, q3) = (row[100], row[200], row[300]);
        let total = row[0] - row[399];
        assert!((row[0] - q1) / total > 0.12 && (q3 - row[399]) / total > 0.12, "even: {} {q1} {mid} {q3} {}", row[0], row[399]);
        // No crease: the slope changes gradually everywhere.
        let d: Vec<f32> = row.windows(2).map(|w| w[1] - w[0]).collect();
        let jump = d.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(jump < 0.05, "slope jumps by {jump} levels");
    }

    #[test]
    fn a_colour_fading_out_stays_that_colour() {
        let p = params(&[("start", Param::Colour([1.0, 0.0, 0.0, 1.0])), ("end", Param::Colour([1.0, 0.0, 0.0, 0.0])), ("grain", Param::Float(0.0))]);
        let f = draw(&p, 100, 1);
        for px in &f.pixels {
            if px[3] > 0.01 {
                let r = px[0] / px[3];
                assert!(r > 0.95 && px[1] / px[3] < 0.05, "{px:?}");
            }
        }
    }

    #[test]
    fn radial_light_has_no_edge() {
        let p = params(&[("grain", Param::Float(0.0)), ("radial", Param::Bool(true)), ("radius", Param::Float(0.3)), ("start", Param::Colour([0.3, 0.2, 0.8, 1.0])), ("end", Param::Colour([0.0, 0.0, 0.02, 1.0]))]);
        let f = draw(&p, 200, 200);
        let row: Vec<f32> = (100..200).map(|x| f.pixels[100 * 200 + x][2]).collect();
        let d: Vec<f32> = row.windows(2).map(|w| w[1] - w[0]).collect();
        // Darkening all the way out, with no sudden change of slope.
        assert!(d.iter().all(|v| *v <= 1e-6));
        let jump = d.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(jump < 0.004, "{jump}");
    }

    /// Two vivid colours meet in a vivid middle, not a grey one.
    #[test]
    fn vivid_pairs_stay_vivid_in_the_middle() {
        let p = params(&[("start", Param::Colour([0.9, 0.25, 0.1, 1.0])), ("end", Param::Colour([0.1, 0.3, 0.9, 1.0])), ("grain", Param::Float(0.0))]);
        let f = draw(&p, 101, 1);
        let chroma = |px: [f32; 4]| {
            let l = to_oklab([px[0], px[1], px[2]]);
            l[1].hypot(l[2])
        };
        let ends = chroma(f.pixels[0]).min(chroma(f.pixels[100]));
        assert!(chroma(f.pixels[50]) > 0.8 * ends, "middle chroma {} vs ends {ends}", chroma(f.pixels[50]));
    }

    #[test]
    fn more_stops_pass_through_each_colour() {
        let p = params(&[("grain", Param::Float(0.0)), ("stop0", Param::Colour([1.0, 0.0, 0.0, 1.0])), ("stop1", Param::Colour([0.0, 1.0, 0.0, 1.0])), ("stop2", Param::Colour([0.0, 0.0, 1.0, 1.0]))]);
        let f = draw(&p, 201, 1);
        let mid = f.pixels[100];
        assert!(mid[1] > 0.9 && mid[0] < 0.1 && mid[2] < 0.1, "{mid:?}");
    }
}
