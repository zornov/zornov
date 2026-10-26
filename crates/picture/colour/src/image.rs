// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The image type every CPU colour tool works on.

/// A linear-light, premultiplied-alpha RGBA image with `f32` channels.
///
/// "Linear" means the RGB values are proportional to light in the working
/// primaries (Rec. 709 unless a [`crate::Grade`] says otherwise); 1.0 is
/// diffuse white, values above 1.0 are allowed (highlights) and small
/// negative values may appear after gamut conversions. "Premultiplied"
/// means colour has already been multiplied by alpha, as compositors want.
///
/// Pixels are stored row by row, top row first. `render` adapts its own
/// frame buffers to and from this type.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

impl Image {
    /// A transparent black image.
    pub fn new(width: u32, height: u32) -> Image {
        Image { width, height, pixels: vec![[0.0; 4]; width as usize * height as usize] }
    }

    /// Builds an image by calling `f(x, y)` for every pixel.
    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> [f32; 4]) -> Image {
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.push(f(x, y));
            }
        }
        Image { width, height, pixels }
    }

    /// Wraps existing pixel data. Panics if the length does not match.
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<[f32; 4]>) -> Image {
        assert_eq!(pixels.len(), width as usize * height as usize, "pixel count mismatch");
        Image { width, height, pixels }
    }

    pub fn len(&self) -> usize {
        self.pixels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }

    pub fn get(&self, x: u32, y: u32) -> [f32; 4] {
        self.pixels[y as usize * self.width as usize + x as usize]
    }

    pub fn set(&mut self, x: u32, y: u32, p: [f32; 4]) {
        let w = self.width as usize;
        self.pixels[y as usize * w + x as usize] = p;
    }
}

/// Removes premultiplication. Fully transparent pixels stay black, because
/// they carry no colour to recover.
#[inline]
pub fn unpremultiply(p: [f32; 4]) -> [f32; 4] {
    let a = p[3];
    if a > 0.0 && a != 1.0 {
        [p[0] / a, p[1] / a, p[2] / a, a]
    } else if a > 0.0 {
        p
    } else {
        [0.0, 0.0, 0.0, a]
    }
}

/// Re-applies premultiplication after an operation on straight colour.
#[inline]
pub fn premultiply(p: [f32; 4]) -> [f32; 4] {
    let a = p[3];
    if a == 1.0 { p } else { [p[0] * a, p[1] * a, p[2] * a, a] }
}

/// Runs `f(y, row)` over every row, spread across the machine's cores.
/// The CPU grading path is the reference implementation and is also used
/// for exports without a GPU, so it is worth parallelising.
pub(crate) fn for_each_row(
    pixels: &mut [[f32; 4]],
    width: usize,
    f: impl Fn(usize, &mut [[f32; 4]]) + Sync,
) {
    if width == 0 || pixels.is_empty() {
        return;
    }
    let rows = pixels.len() / width;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(rows);
    if threads <= 1 || pixels.len() < 64 * 1024 {
        for (y, row) in pixels.chunks_mut(width).enumerate() {
            f(y, row);
        }
        return;
    }
    let rows_per = rows.div_ceil(threads);
    std::thread::scope(|s| {
        for (i, chunk) in pixels.chunks_mut(rows_per * width).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (j, row) in chunk.chunks_mut(width).enumerate() {
                    f(i * rows_per + j, row);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply_round_trip() {
        let p = [0.2, 0.4, 0.1, 0.5];
        let s = unpremultiply(p);
        assert_eq!(s, [0.4, 0.8, 0.2, 0.5]);
        assert_eq!(premultiply(s), p);
        assert_eq!(unpremultiply([0.3, 0.3, 0.3, 0.0]), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn rows_visit_everything() {
        let mut img = Image::new(300, 400);
        for_each_row(&mut img.pixels, 300, |y, row| {
            for (x, p) in row.iter_mut().enumerate() {
                *p = [x as f32, y as f32, 0.0, 1.0];
            }
        });
        assert_eq!(img.get(299, 399), [299.0, 399.0, 0.0, 1.0]);
        assert_eq!(img.get(7, 123), [7.0, 123.0, 0.0, 1.0]);
    }
}
