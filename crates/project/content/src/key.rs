// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic chroma key for green-screen items.
//!
//! The key colour is detected from the picture itself (the border is
//! mostly backdrop), then every pixel is classified in chroma space
//! relative to that colour: pixels whose chroma points the same way as the
//! key and is at least as strong are backdrop, pixels with little chroma
//! (grey clothing, white, black) or a different hue are subject. Edges get
//! a soft ramp, edge colours are un-mixed from the backdrop, and remaining
//! green spill is clamped. Everything is in sRGB with BT.601 full-range
//! chroma, which is plenty for stills and the result stays editable through
//! [`KeySettings`].

pub use crate::item::KeySettings;
use studio_media::rgba::RgbaImage;

/// Chroma (Cb, Cr) of an sRGB pixel, each in −0.5..0.5.
fn chroma(r: f32, g: f32, b: f32) -> (f32, f32) {
    let cb = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
    let cr = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
    (cb, cr)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `true` when the picture was already keyed upstream: most of its border
/// is transparent.
pub fn is_prekeyed(img: &RgbaImage) -> bool {
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 8 || h < 8 {
        return false;
    }
    let band = ((w.min(h) as f32) * 0.04).ceil().max(2.0) as usize;
    let (mut total, mut clear) = (0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if x >= band && x < w - band && y >= band && y < h - band {
                continue;
            }
            total += 1;
            if img.pixels[(y * w + x) * 4 + 3] < 16 {
                clear += 1;
            }
        }
    }
    total > 0 && clear * 2 > total
}

/// The colour left under fully transparent pixels of a pre-keyed picture
/// (the backdrop as the upstream keyer saw it), or `None` when there are
/// too few such pixels.
pub fn residual_colour(img: &RgbaImage) -> Option<[u8; 3]> {
    let mut samples: Vec<[u8; 3]> = img.pixels.chunks_exact(4).filter(|p| p[3] == 0).step_by(7).map(|p| [p[0], p[1], p[2]]).collect();
    if samples.len() < 64 {
        return None;
    }
    let mut out = [0u8; 3];
    for c in 0..3 {
        samples.sort_by_key(|p| p[c]);
        out[c] = samples[samples.len() / 2][c];
    }
    Some(out)
}

/// Finds the backdrop colour of a green- (or blue-) screen still.
///
/// Returns `None` when the border does not look like a backdrop: fewer than
/// 40 % of border pixels share one strongly saturated hue. A picture that
/// was already keyed upstream reports the colour left under its
/// transparent pixels instead.
pub fn detect_key(img: &RgbaImage) -> Option<[u8; 3]> {
    if is_prekeyed(img) {
        return residual_colour(img);
    }
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 8 || h < 8 {
        return None;
    }
    let band = ((w.min(h) as f32) * 0.04).ceil().max(2.0) as usize;
    let mut samples: Vec<[u8; 3]> = Vec::new();
    let mut total = 0usize;
    for y in 0..h {
        for x in 0..w {
            if x >= band && x < w - band && y >= band && y < h - band {
                continue;
            }
            let i = (y * w + x) * 4;
            let p = &img.pixels[i..i + 4];
            if p[3] < 128 {
                continue;
            }
            total += 1;
            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
            let dominant_green = g > r + 40 && g > b + 40;
            let dominant_blue = b > r + 40 && b > g + 40;
            if dominant_green || dominant_blue {
                samples.push([p[0], p[1], p[2]]);
            }
        }
    }
    if total == 0 || (samples.len() as f32) < 0.4 * total as f32 {
        return None;
    }
    // Keep the majority hue family, then take the per-channel median.
    let greens = samples.iter().filter(|p| p[1] > p[2]).count();
    let want_green = greens * 2 >= samples.len();
    let mut keep: Vec<[u8; 3]> = samples.into_iter().filter(|p| (p[1] > p[2]) == want_green).collect();
    let mut out = [0u8; 3];
    for c in 0..3 {
        keep.sort_by_key(|p| p[c]);
        out[c] = keep[keep.len() / 2][c];
    }
    Some(out)
}

/// Keys `img` with `settings`, returning a straight-alpha picture.
pub fn apply(img: &RgbaImage, settings: &KeySettings) -> RgbaImage {
    let kr = settings.key[0] as f32 / 255.0;
    let kg = settings.key[1] as f32 / 255.0;
    let kb = settings.key[2] as f32 / 255.0;
    let (kcb, kcr) = chroma(kr, kg, kb);
    let mag = (kcb * kcb + kcr * kcr).sqrt().max(1e-3);
    let (ux, uy) = (kcb / mag, kcr / mag);
    let tol = settings.tolerance.clamp(0.0, 1.0);
    let soft = settings.softness.clamp(0.02, 1.0);
    // How far along the key direction a pixel must be to count as backdrop,
    // and how far across (hue difference) it may stray.
    let along_lo = 0.65 - 0.5 * tol;
    let along_hi = along_lo + 0.45 * soft;
    let across_hi = 0.25 + 0.45 * tol;
    let across_lo = (across_hi - 0.5 * soft).max(0.02);
    let spill = settings.spill.clamp(0.0, 1.0);
    let green_key = kg >= kb;

    let mut out = RgbaImage::new(img.width, img.height);
    for (src, dst) in img.pixels.chunks_exact(4).zip(out.pixels.chunks_exact_mut(4)) {
        let r = src[0] as f32 / 255.0;
        let g = src[1] as f32 / 255.0;
        let b = src[2] as f32 / 255.0;
        let (cb, cr) = chroma(r, g, b);
        let along = (cb * ux + cr * uy) / mag;
        let across = (cb * uy - cr * ux).abs() / mag;
        // Opaque when the chroma is weak or points elsewhere.
        let a_along = 1.0 - smoothstep(along_lo, along_hi, along);
        let a_across = smoothstep(across_lo, across_hi, across);
        let alpha = a_along.max(a_across) * (src[3] as f32 / 255.0);
        if alpha <= 0.002 {
            dst.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        // Un-mix the backdrop from edge pixels: observed = a·fg + (1−a)·key.
        let (mut fr, mut fg, mut fb) = if alpha < 0.999 {
            (
                ((r - (1.0 - alpha) * kr) / alpha).clamp(0.0, 1.0),
                ((g - (1.0 - alpha) * kg) / alpha).clamp(0.0, 1.0),
                ((b - (1.0 - alpha) * kb) / alpha).clamp(0.0, 1.0),
            )
        } else {
            (r, g, b)
        };
        // Spill suppression: the key channel may not exceed the others.
        if spill > 0.0 {
            if green_key {
                let limit = fr.max(fb);
                if fg > limit {
                    let excess = (fg - limit) * spill;
                    fg -= excess;
                    // Give a little of the removed brightness back so edges do
                    // not darken.
                    fr = (fr + excess * 0.3).min(1.0);
                    fb = (fb + excess * 0.3).min(1.0);
                }
            } else {
                let limit = fr.max(fg);
                if fb > limit {
                    let excess = (fb - limit) * spill;
                    fb -= excess;
                    fr = (fr + excess * 0.3).min(1.0);
                    fg = (fg + excess * 0.3).min(1.0);
                }
            }
        }
        dst[0] = (fr * 255.0).round() as u8;
        dst[1] = (fg * 255.0).round() as u8;
        dst[2] = (fb * 255.0).round() as u8;
        dst[3] = (alpha * 255.0).round() as u8;
    }
    out
}

/// Cleans the soft edge of a picture that already carries alpha: partial
/// pixels are un-mixed from the backdrop colour and despilled, fully
/// opaque pixels are left alone (a green subject stays green), and
/// fully transparent pixels are zeroed so nothing leaks through filtering.
pub fn refine_edges(img: &RgbaImage, settings: &KeySettings) -> RgbaImage {
    let kr = settings.key[0] as f32 / 255.0;
    let kg = settings.key[1] as f32 / 255.0;
    let kb = settings.key[2] as f32 / 255.0;
    let spill = settings.spill.clamp(0.0, 1.0);
    let green_key = kg >= kb;
    let mut out = img.clone();
    for px in out.pixels.chunks_exact_mut(4) {
        let a = px[3];
        if a == 255 {
            continue;
        }
        if a < 2 {
            px.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let alpha = a as f32 / 255.0;
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        // Only un-mix when the pixel actually leans towards the backdrop;
        // a clean edge pixel keeps its colour.
        let lean = (g - r.max(b)) > 0.02 && green_key || (b - r.max(g)) > 0.02 && !green_key;
        let (mut fr, mut fg, mut fb) = if lean {
            (
                ((r - (1.0 - alpha) * kr) / alpha).clamp(0.0, 1.0),
                ((g - (1.0 - alpha) * kg) / alpha).clamp(0.0, 1.0),
                ((b - (1.0 - alpha) * kb) / alpha).clamp(0.0, 1.0),
            )
        } else {
            (r, g, b)
        };
        if spill > 0.0 {
            if green_key {
                let limit = fr.max(fb);
                if fg > limit {
                    let excess = (fg - limit) * spill;
                    fg -= excess;
                    fr = (fr + excess * 0.3).min(1.0);
                    fb = (fb + excess * 0.3).min(1.0);
                }
            } else {
                let limit = fr.max(fg);
                if fb > limit {
                    let excess = (fb - limit) * spill;
                    fb -= excess;
                    fr = (fr + excess * 0.3).min(1.0);
                    fg = (fg + excess * 0.3).min(1.0);
                }
            }
        }
        px[0] = (fr * 255.0).round() as u8;
        px[1] = (fg * 255.0).round() as u8;
        px[2] = (fb * 255.0).round() as u8;
    }
    out
}

/// Detects the backdrop and keys the picture with default settings; a
/// picture keyed upstream gets its edges refined instead. `None` when the
/// picture is opaque and not on a green or blue screen.
pub fn auto(img: &RgbaImage) -> Option<(RgbaImage, KeySettings)> {
    if is_prekeyed(img) {
        let key = residual_colour(img).unwrap_or([0, 255, 0]);
        let settings = KeySettings { key, ..KeySettings::default() };
        return Some((refine_edges(img, &settings), settings));
    }
    let key = detect_key(img)?;
    let settings = KeySettings { key, ..KeySettings::default() };
    Some((apply(img, &settings), settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: [u8; 4] = [38, 196, 52, 255];

    fn screen(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for p in img.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&GREEN);
        }
        img
    }

    fn set(img: &mut RgbaImage, x: u32, y: u32, rgba: [u8; 4]) {
        let i = ((y * img.width + x) * 4) as usize;
        img.pixels[i..i + 4].copy_from_slice(&rgba);
    }

    fn get(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn detects_green_backdrop_and_rejects_photos() {
        let img = screen(64, 64);
        let key = detect_key(&img).unwrap();
        assert!(key[1] > key[0] + 40 && key[1] > key[2] + 40);
        let mut photo = RgbaImage::new(64, 64);
        for p in photo.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&[120, 110, 100, 255]);
        }
        assert_eq!(detect_key(&photo), None);
    }

    #[test]
    fn keys_backdrop_keeps_subject_and_neutrals() {
        let mut img = screen(64, 64);
        // Subject: grey shirt, skin, white, black, red — all must stay opaque.
        let colours = [[128, 128, 128, 255], [224, 172, 140, 255], [255, 255, 255, 255], [8, 8, 8, 255], [200, 30, 30, 255], [60, 90, 70, 255]];
        for (i, c) in colours.iter().enumerate() {
            for y in 20..44 {
                for x in 0..8 {
                    set(&mut img, 12 + i as u32 * 8 + x, y, *c);
                }
            }
        }
        let (out, settings) = auto(&img).unwrap();
        assert_eq!(get(&out, 2, 2)[3], 0, "backdrop must be fully transparent");
        for i in 0..colours.len() {
            let p = get(&out, 16 + i as u32 * 8, 30);
            assert!(p[3] > 250, "colour {:?} became {:?}", colours[i], p);
        }
        // Grey stays grey (no spill clamping on neutrals).
        let grey = get(&out, 16, 30);
        assert!((grey[0] as i32 - grey[1] as i32).abs() <= 2);
        assert!(settings.key[1] > settings.key[0]);
    }

    #[test]
    fn edge_pixels_are_soft_and_unmixed() {
        // Half-mixed pixel of white subject and green backdrop.
        let mut img = screen(32, 32);
        for y in 8..24 {
            for x in 8..24 {
                set(&mut img, x, y, [255, 255, 255, 255]);
            }
            let mix = [(255u32 + GREEN[0] as u32) / 2, (255u32 + GREEN[1] as u32) / 2, (255u32 + GREEN[2] as u32) / 2];
            set(&mut img, 24, y, [mix[0] as u8, mix[1] as u8, mix[2] as u8, 255]);
        }
        let (out, _) = auto(&img).unwrap();
        let edge = get(&out, 24, 16);
        assert!(edge[3] > 20 && edge[3] < 240, "edge alpha {:?}", edge);
        // Un-mixed edge colour leans white and is not greener than it is red.
        assert!(edge[1] as u32 <= edge[0].max(edge[2]) as u32 + 2, "spill left: {edge:?}");
    }

    #[test]
    fn prekeyed_pictures_get_their_edges_cleaned() {
        // Transparent border carrying a dark green residual, an opaque green
        // subject, and a half-mixed edge pixel.
        let mut img = RgbaImage::new(64, 64);
        for p in img.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&[29, 78, 67, 0]);
        }
        for y in 20..44 {
            for x in 20..44 {
                set(&mut img, x, y, [40, 180, 60, 255]);
            }
            set(&mut img, 44, y, [37, 89, 85, 134]);
        }
        assert!(is_prekeyed(&img));
        assert_eq!(residual_colour(&img), Some([29, 78, 67]));
        let (out, s) = auto(&img).unwrap();
        assert_eq!(s.key, [29, 78, 67]);
        assert_eq!(get(&out, 30, 30), [40, 180, 60, 255], "opaque green subject untouched");
        assert_eq!(get(&out, 2, 2), [0, 0, 0, 0], "residual cleared");
        let edge = get(&out, 44, 30);
        assert_eq!(edge[3], 134, "existing alpha kept");
        assert!(edge[1] as u32 <= edge[0].max(edge[2]) as u32 + 1, "edge despilled: {edge:?}");
    }

    #[test]
    fn blue_screen_works_too() {
        let mut img = RgbaImage::new(32, 32);
        for p in img.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&[20, 40, 220, 255]);
        }
        set(&mut img, 16, 16, [200, 200, 200, 255]);
        let (out, s) = auto(&img).unwrap();
        assert!(s.key[2] > s.key[1]);
        assert_eq!(get(&out, 1, 1)[3], 0);
        assert!(get(&out, 16, 16)[3] > 250);
    }
}
