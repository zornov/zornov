// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! PNG read/write for review sheets and golden tests. Images are stored
//! as 8-bit sRGB with straight alpha; the in-memory type is linear
//! premultiplied, so both directions convert.

use std::path::Path;

use crate::rgba::{linear_to_srgb, srgb_to_linear};
use crate::Image;

/// Writes an image as RGBA8 PNG.
pub fn write_png(path: &Path, img: &Image) -> std::io::Result<()> {
    let mut data = Vec::with_capacity(img.pixels.len() * 4);
    for p in &img.pixels {
        let a = p[3].clamp(0.0, 1.0);
        let un = |c: f32| if a > 0.0 { (c / a).clamp(0.0, 1.0) } else { 0.0 };
        data.push((linear_to_srgb(un(p[0])) * 255.0).round() as u8);
        data.push((linear_to_srgb(un(p[1])) * 255.0).round() as u8);
        data.push((linear_to_srgb(un(p[2])) * 255.0).round() as u8);
        data.push((a * 255.0).round() as u8);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    w.write_image_data(&data).map_err(std::io::Error::other)?;
    Ok(())
}

/// Reads an RGBA/RGB/grey PNG into a linear premultiplied image.
pub fn read_png(path: &Path) -> std::io::Result<Image> {
    let file = std::fs::File::open(path)?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(std::io::Error::other)?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(std::io::Error::other)?;
    let bytes = &buf[..info.buffer_size()];
    let (w, h) = (info.width, info.height);
    let chans = info.color_type.samples();
    let bps = match info.bit_depth {
        png::BitDepth::Sixteen => 2,
        _ => 1,
    };
    let get = |i: usize| -> f32 {
        if bps == 2 { u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]]) as f32 / 65535.0 } else { bytes[i] as f32 / 255.0 }
    };
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for i in 0..(w * h) as usize {
        let base = i * chans;
        let (r, g, b, a) = match info.color_type {
            png::ColorType::Rgba => (get(base), get(base + 1), get(base + 2), get(base + 3)),
            png::ColorType::Rgb => (get(base), get(base + 1), get(base + 2), 1.0),
            png::ColorType::GrayscaleAlpha => (get(base), get(base), get(base), get(base + 1)),
            _ => (get(base), get(base), get(base), 1.0),
        };
        pixels.push([srgb_to_linear(r) * a, srgb_to_linear(g) * a, srgb_to_linear(b) * a, a]);
    }
    Ok(Image::from_pixels(w, h, pixels))
}

/// Mean absolute difference per channel (0..1) between two images of the
/// same size; `None` when sizes differ.
pub fn mean_abs_diff(a: &Image, b: &Image) -> Option<f32> {
    if a.width != b.width || a.height != b.height {
        return None;
    }
    let n = (a.pixels.len() * 4).max(1) as f32;
    let sum: f32 = a.pixels.iter().zip(&b.pixels).map(|(p, q)| (0..4).map(|c| (p[c] - q[c]).abs()).sum::<f32>()).sum();
    Some(sum / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let img = Image::from_fn(4, 3, |x, y| {
            let a = if x == 0 { 0.0 } else { 1.0 };
            [srgb_to_linear(x as f32 / 3.0) * a, srgb_to_linear(y as f32 / 2.0) * a, 0.5 * a, a]
        });
        let dir = std::env::temp_dir().join("studio-text-png-test");
        let p = dir.join("rt.png");
        write_png(&p, &img).unwrap();
        let back = read_png(&p).unwrap();
        assert!(mean_abs_diff(&img, &back).unwrap() < 0.01);
    }
}
