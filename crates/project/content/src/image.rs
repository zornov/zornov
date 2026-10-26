// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pixel helpers on `RgbaImage`: a high-quality separable Lanczos resample
//! on premultiplied alpha, cropping, compositing, a checkerboard and a tiny
//! built-in label font for contact sheets.

use studio_media::rgba::RgbaImage;

/// Target size that fits `w × h` inside a `max_side` square without
/// upscaling, keeping the aspect ratio (never smaller than 1 px).
pub fn fit(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest <= max_side || longest == 0 {
        return (w.max(1), h.max(1));
    }
    let s = max_side as f64 / longest as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

fn lanczos3(x: f32) -> f32 {
    const A: f32 = 3.0;
    if x == 0.0 {
        return 1.0;
    }
    if x.abs() >= A {
        return 0.0;
    }
    let px = std::f32::consts::PI * x;
    A * px.sin() * (px / A).sin() / (px * px)
}

/// Per-output-pixel filter taps for one axis. For downscales the kernel is
/// widened by the scale factor so every source pixel contributes (this is
/// what keeps downscales free of jaggies).
fn taps(src_len: u32, dst_len: u32) -> Vec<(usize, Vec<f32>)> {
    let scale = src_len as f32 / dst_len as f32;
    let support = if scale > 1.0 { 3.0 * scale } else { 3.0 };
    let inv = if scale > 1.0 { 1.0 / scale } else { 1.0 };
    (0..dst_len)
        .map(|o| {
            let centre = (o as f32 + 0.5) * scale;
            let lo = ((centre - support).floor().max(0.0)) as usize;
            let hi = ((centre + support).ceil().min(src_len as f32)) as usize;
            let mut ws: Vec<f32> = (lo..hi).map(|i| lanczos3((i as f32 + 0.5 - centre) * inv)).collect();
            let sum: f32 = ws.iter().sum();
            if sum.abs() > 1e-6 {
                for w in &mut ws {
                    *w /= sum;
                }
            }
            (lo, ws)
        })
        .collect()
}

/// Resamples to `dst_w × dst_h` with a Lanczos-3 filter on premultiplied
/// alpha. Works for up- and downscaling.
pub fn resize(img: &RgbaImage, dst_w: u32, dst_h: u32) -> RgbaImage {
    let (sw, sh) = (img.width as usize, img.height as usize);
    if (dst_w as usize, dst_h as usize) == (sw, sh) {
        return img.clone();
    }
    let (dw, dh) = (dst_w.max(1) as usize, dst_h.max(1) as usize);
    // Premultiply into f32.
    let mut src = vec![0f32; sw * sh * 4];
    for (i, px) in img.pixels.chunks_exact(4).enumerate() {
        let a = px[3] as f32 / 255.0;
        src[i * 4] = px[0] as f32 * a;
        src[i * 4 + 1] = px[1] as f32 * a;
        src[i * 4 + 2] = px[2] as f32 * a;
        src[i * 4 + 3] = px[3] as f32;
    }
    // Horizontal pass: sw×sh -> dw×sh.
    let hx = taps(img.width, dst_w);
    let mut mid = vec![0f32; dw * sh * 4];
    for y in 0..sh {
        let row = &src[y * sw * 4..(y + 1) * sw * 4];
        let out = &mut mid[y * dw * 4..(y + 1) * dw * 4];
        for (o, (lo, ws)) in hx.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (k, w) in ws.iter().enumerate() {
                let i = (lo + k) * 4;
                acc[0] += row[i] * w;
                acc[1] += row[i + 1] * w;
                acc[2] += row[i + 2] * w;
                acc[3] += row[i + 3] * w;
            }
            out[o * 4..o * 4 + 4].copy_from_slice(&acc);
        }
    }
    // Vertical pass: dw×sh -> dw×dh.
    let vy = taps(img.height, dst_h);
    let mut dst = RgbaImage::new(dw as u32, dh as u32);
    for (o, (lo, ws)) in vy.iter().enumerate() {
        for x in 0..dw {
            let mut acc = [0f32; 4];
            for (k, w) in ws.iter().enumerate() {
                let i = ((lo + k) * dw + x) * 4;
                acc[0] += mid[i] * w;
                acc[1] += mid[i + 1] * w;
                acc[2] += mid[i + 2] * w;
                acc[3] += mid[i + 3] * w;
            }
            let a = acc[3].clamp(0.0, 255.0);
            let p = &mut dst.pixels[(o * dw + x) * 4..(o * dw + x) * 4 + 4];
            if a > 0.0 {
                let inv = 255.0 / a;
                p[0] = (acc[0] * inv).round().clamp(0.0, 255.0) as u8;
                p[1] = (acc[1] * inv).round().clamp(0.0, 255.0) as u8;
                p[2] = (acc[2] * inv).round().clamp(0.0, 255.0) as u8;
            }
            p[3] = a.round() as u8;
        }
    }
    dst
}

/// Resizes so the longest side is at most `max_side` (no upscaling).
pub fn resize_to_fit(img: &RgbaImage, max_side: u32) -> RgbaImage {
    let (w, h) = fit(img.width, img.height, max_side);
    resize(img, w, h)
}

/// Inclusive-exclusive bounds `(x0, y0, x1, y1)` of pixels with alpha above
/// `threshold`, or `None` when the picture is fully transparent.
pub fn alpha_bbox(img: &RgbaImage, threshold: u8) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (img.width as usize, img.height as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        let row = &img.pixels[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            if row[x * 4 + 3] > threshold {
                x0 = x0.min(x);
                x1 = x1.max(x + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0).then_some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
}

/// Copies the rectangle `(x0, y0)..(x1, y1)` (clamped to the picture).
pub fn crop(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> RgbaImage {
    let x1 = x1.min(img.width);
    let y1 = y1.min(img.height);
    let x0 = x0.min(x1);
    let y0 = y0.min(y1);
    let mut out = RgbaImage::new(x1 - x0, y1 - y0);
    let sw = img.width as usize * 4;
    let dw = out.width as usize * 4;
    for (dy, y) in (y0..y1).enumerate() {
        let s = y as usize * sw + x0 as usize * 4;
        out.pixels[dy * dw..(dy + 1) * dw].copy_from_slice(&img.pixels[s..s + dw]);
    }
    out
}

/// Crops to the opaque content with `margin` pixels of padding.
pub fn crop_to_content(img: &RgbaImage, threshold: u8, margin: u32) -> RgbaImage {
    match alpha_bbox(img, threshold) {
        Some((x0, y0, x1, y1)) => {
            crop(img, x0.saturating_sub(margin), y0.saturating_sub(margin), x1 + margin, y1 + margin)
        }
        None => img.clone(),
    }
}

/// Composites `src` over `dst` at `(x, y)` (straight alpha, "over").
pub fn blit(dst: &mut RgbaImage, src: &RgbaImage, x: i64, y: i64) {
    for sy in 0..src.height as i64 {
        let dy = y + sy;
        if dy < 0 || dy >= dst.height as i64 {
            continue;
        }
        for sx in 0..src.width as i64 {
            let dx = x + sx;
            if dx < 0 || dx >= dst.width as i64 {
                continue;
            }
            let s = ((sy as usize) * src.width as usize + sx as usize) * 4;
            let d = ((dy as usize) * dst.width as usize + dx as usize) * 4;
            let sa = src.pixels[s + 3] as f32 / 255.0;
            if sa <= 0.0 {
                continue;
            }
            let da = dst.pixels[d + 3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            for c in 0..3 {
                let sc = src.pixels[s + c] as f32;
                let dc = dst.pixels[d + c] as f32;
                let v = if oa > 0.0 { (sc * sa + dc * da * (1.0 - sa)) / oa } else { 0.0 };
                dst.pixels[d + c] = v.round().clamp(0.0, 255.0) as u8;
            }
            dst.pixels[d + 3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Fills a rectangle with an opaque colour (clamped).
pub fn fill_rect(img: &mut RgbaImage, x0: i64, y0: i64, w: u32, h: u32, rgba: [u8; 4]) {
    for y in y0.max(0)..(y0 + h as i64).min(img.height as i64) {
        for x in x0.max(0)..(x0 + w as i64).min(img.width as i64) {
            let i = (y as usize * img.width as usize + x as usize) * 4;
            img.pixels[i..i + 4].copy_from_slice(&rgba);
        }
    }
}

/// A light/dark checkerboard, the usual way to show transparency.
pub fn checkerboard(w: u32, h: u32, cell: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let dark = ((x / cell) + (y / cell)) % 2 == 1;
            let v = if dark { 0x9c } else { 0xb8 };
            let i = ((y * w + x) * 4) as usize;
            img.pixels[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    img
}

/// Fraction of pixels whose alpha is above `threshold`.
pub fn coverage(img: &RgbaImage, threshold: u8) -> f64 {
    let n = img.pixels.chunks_exact(4).filter(|p| p[3] > threshold).count();
    n as f64 / (img.width as f64 * img.height as f64).max(1.0)
}

/// `true` when every pixel is fully opaque.
pub fn is_opaque(img: &RgbaImage) -> bool {
    img.pixels.chunks_exact(4).all(|p| p[3] == 255)
}

// A 5×7 bitmap font of our own for labels on review sheets. Each glyph is
// seven rows of five bits, most significant bit on the left.
const GLYPHS: &[(char, [u8; 7])] = &[
    (' ', [0, 0, 0, 0, 0, 0, 0]),
    ('A', [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11]),
    ('B', [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e]),
    ('C', [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e]),
    ('D', [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e]),
    ('E', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f]),
    ('F', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10]),
    ('G', [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f]),
    ('H', [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11]),
    ('I', [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e]),
    ('J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c]),
    ('K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f]),
    ('M', [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ('N', [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
    ('O', [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
    ('P', [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10]),
    ('Q', [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d]),
    ('R', [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11]),
    ('S', [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e]),
    ('T', [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
    ('V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04]),
    ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11]),
    ('X', [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11]),
    ('Y', [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04]),
    ('Z', [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f]),
    ('0', [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e]),
    ('1', [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e]),
    ('2', [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f]),
    ('3', [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e]),
    ('4', [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02]),
    ('5', [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e]),
    ('6', [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e]),
    ('7', [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e]),
    ('9', [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c]),
    ('-', [0, 0, 0, 0x1f, 0, 0, 0]),
    ('.', [0, 0, 0, 0, 0, 0x0c, 0x0c]),
    (':', [0, 0x0c, 0x0c, 0, 0x0c, 0x0c, 0]),
    ('&', [0x08, 0x14, 0x14, 0x08, 0x15, 0x12, 0x0d]),
    ('/', [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10]),
    ('%', [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03]),
    ('(', [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02]),
    (')', [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08]),
    ('?', [0x0e, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04]),
];

/// Width in pixels of `text` drawn by [`draw_label`] at `scale`.
pub fn label_width(text: &str, scale: u32) -> u32 {
    text.chars().count() as u32 * 6 * scale
}

/// Draws upper-case text with the built-in 5×7 font. Lower-case letters are
/// drawn as capitals; unknown characters as `?`.
pub fn draw_label(img: &mut RgbaImage, x: i64, y: i64, text: &str, scale: u32, rgba: [u8; 4]) {
    let mut cx = x;
    for ch in text.chars() {
        let up = ch.to_ascii_uppercase();
        let rows = GLYPHS.iter().find(|(c, _)| *c == up).map(|(_, r)| *r).unwrap_or(GLYPHS.last().unwrap().1);
        for (ry, bits) in rows.iter().enumerate() {
            for bx in 0..5 {
                if bits & (0x10 >> bx) != 0 {
                    fill_rect(img, cx + (bx * scale) as i64, y + (ry as u32 * scale) as i64, scale, scale, rgba);
                }
            }
        }
        cx += 6 * scale as i64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for p in img.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&rgba);
        }
        img
    }

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit(2000, 1000, 500), (500, 250));
        assert_eq!(fit(100, 300, 500), (100, 300));
        assert_eq!(fit(1, 4000, 512), (1, 512));
    }

    #[test]
    fn resize_preserves_flat_colour_and_alpha() {
        let img = solid(97, 61, [200, 100, 50, 255]);
        let out = resize(&img, 23, 17);
        assert_eq!((out.width, out.height), (23, 17));
        for p in out.pixels.chunks_exact(4) {
            assert!((p[0] as i32 - 200).abs() <= 1 && (p[1] as i32 - 100).abs() <= 1 && (p[2] as i32 - 50).abs() <= 1 && p[3] == 255, "{p:?}");
        }
        let clear = solid(40, 40, [0, 0, 0, 0]);
        assert!(resize(&clear, 10, 10).pixels.iter().all(|&v| v == 0));
    }

    #[test]
    fn downscale_of_edge_is_smooth_not_jagged() {
        // Left half opaque red, right half transparent; a 8x downscale must
        // give an intermediate alpha column, not a hard step.
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let i = (y * 64 + x) * 4;
                if x < 33 {
                    img.pixels[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
                }
            }
        }
        let out = resize(&img, 8, 8);
        let a: Vec<u8> = (0..8).map(|x| out.pixels[(3 * 8 + x) * 4 + 3]).collect();
        assert_eq!(a[0], 255);
        assert_eq!(a[7], 0);
        assert!(a[4] > 20 && a[4] < 235, "{a:?}");
        // Colour stays red where alpha is partial (premultiplied resample).
        assert!(out.pixels[(3 * 8 + 4) * 4] > 240);
    }

    #[test]
    fn bbox_crop_and_blit() {
        let mut img = RgbaImage::new(10, 10);
        fill_rect(&mut img, 3, 4, 2, 3, [1, 2, 3, 255]);
        assert_eq!(alpha_bbox(&img, 0), Some((3, 4, 5, 7)));
        let c = crop_to_content(&img, 0, 1);
        assert_eq!((c.width, c.height), (4, 5));
        let mut dst = checkerboard(10, 10, 2);
        blit(&mut dst, &c, 0, 0);
        assert_eq!(&dst.pixels[((1 * 10 + 1) * 4)..((1 * 10 + 1) * 4 + 4)], &[1, 2, 3, 255]);
        assert_eq!(alpha_bbox(&RgbaImage::new(4, 4), 0), None);
        assert!(is_opaque(&dst));
        assert!((coverage(&img, 0) - 0.06).abs() < 1e-9);
    }

    #[test]
    fn labels_draw_something() {
        let mut img = RgbaImage::new(64, 10);
        draw_label(&mut img, 1, 1, "Ok 7", 1, [255, 255, 255, 255]);
        assert!(coverage(&img, 0) > 0.02);
        assert_eq!(label_width("abc", 2), 36);
    }
}
