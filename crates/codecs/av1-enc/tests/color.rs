// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M11: tests for the colour conversion (`color`): known BT.709 values, an independent
//! floating-point reference, the inverse transform, odd sizes, and 16-bit input.
//!
//! The workspace has no other RGB to YUV path to compare with (the AV1 decoder hands out planar
//! samples and leaves colour to its callers), so the reference here is the textbook formula in
//! `f64`, written separately from the implementation's fixed-point matrix.

use studio_av1_enc::color::{from_rgba8, from_rgba16};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
}

fn yuv_of(rgb: [u8; 3], bd: u8, full: bool) -> [u16; 3] {
    let f = from_rgba8(&solid(2, 2, rgb), 2, 2, bd, full);
    [f.planes[0][0], f.planes[1][0], f.planes[2][0]]
}

#[test]
fn bt709_limited_range_primaries() {
    assert_eq!(yuv_of([255, 255, 255], 8, false), [235, 128, 128]);
    assert_eq!(yuv_of([0, 0, 0], 8, false), [16, 128, 128]);
    assert_eq!(yuv_of([255, 0, 0], 8, false), [63, 102, 240]);
    assert_eq!(yuv_of([0, 255, 0], 8, false), [173, 42, 26]);
    assert_eq!(yuv_of([0, 0, 255], 8, false), [32, 240, 118]);
    let g = yuv_of([128, 128, 128], 8, false);
    assert_eq!((g[1], g[2]), (128, 128));
    assert!((g[0] as i32 - 126).abs() <= 1, "mid grey luma {}", g[0]);
}

#[test]
fn ten_bit_and_full_range() {
    assert_eq!(yuv_of([255, 255, 255], 10, false), [940, 512, 512]);
    assert_eq!(yuv_of([0, 0, 0], 10, false), [64, 512, 512]);
    assert_eq!(yuv_of([255, 255, 255], 8, true), [255, 128, 128]);
    assert_eq!(yuv_of([0, 0, 0], 8, true), [0, 128, 128]);
    assert_eq!(yuv_of([255, 255, 255], 10, true), [1023, 512, 512]);
    // full-range chroma reaches the ends for saturated colours (clamped to the code range)
    let r = yuv_of([255, 0, 0], 8, true);
    assert_eq!(r[2], 255);
}

/// The textbook conversion in floating point.
fn reference(r: f64, g: f64, b: f64, bd: u8, full: bool) -> [f64; 3] {
    let (kr, kb) = (0.2126, 0.0722);
    let y = kr * r + (1.0 - kr - kb) * g + kb * b;
    let cb = (b - y) / (2.0 * (1.0 - kb));
    let cr = (r - y) / (2.0 * (1.0 - kr));
    let s = (1u32 << (bd - 8)) as f64;
    if full {
        let m = ((1u32 << bd) - 1) as f64;
        [y * m, 128.0 * s + cb * m, 128.0 * s + cr * m]
    } else {
        [16.0 * s + 219.0 * s * y, 128.0 * s + 224.0 * s * cb, 128.0 * s + 224.0 * s * cr]
    }
}

#[test]
fn matches_the_float_reference_on_random_pixels() {
    let mut rng = Rng(7);
    let (w, h) = (24u32, 18u32);
    for (bd, full) in [(8u8, false), (8, true), (10, false), (10, true)] {
        // 2x2 blocks of one colour, so the chroma average is the reference of that colour
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for by in 0..h / 2 {
            for bx in 0..w / 2 {
                let c = [rng.next() as u8, rng.next() as u8, rng.next() as u8];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = (((2 * by + dy) * w + 2 * bx + dx) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
        }
        let f = from_rgba8(&rgba, w, h, bd, full);
        let max = ((1u32 << bd) - 1) as f64;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 4;
                let want = reference(rgba[i] as f64 / 255.0, rgba[i + 1] as f64 / 255.0, rgba[i + 2] as f64 / 255.0, bd, full);
                let got_y = f.planes[0][y * f.stride[0] + x] as f64;
                assert!((got_y - want[0].round().clamp(0.0, max)).abs() <= 1.0, "bd{bd} full {full} luma ({x},{y}): {got_y} vs {}", want[0]);
                let (cx, cy) = (x / 2, y / 2);
                for p in 1..3 {
                    let got = f.planes[p][cy * f.stride[p] + cx] as f64;
                    assert!((got - want[p].round().clamp(0.0, max)).abs() <= 1.0, "bd{bd} full {full} plane {p} ({cx},{cy}): {got} vs {}", want[p]);
                }
            }
        }
    }
}

#[test]
fn round_trips_through_the_inverse_matrix() {
    // smooth ramps, so the 2x2 chroma average loses nothing
    let (w, h) = (32u32, 32u32);
    let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 8) as u8, (i / w * 8) as u8, 100, 255]).collect();
    let f = from_rgba8(&rgba, w, h, 8, false);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let yy = (f.planes[0][y * f.stride[0] + x] as f64 - 16.0) / 219.0;
            let cb = (f.planes[1][(y / 2) * f.stride[1] + x / 2] as f64 - 128.0) / 224.0;
            let cr = (f.planes[2][(y / 2) * f.stride[2] + x / 2] as f64 - 128.0) / 224.0;
            let r = (yy + 2.0 * (1.0 - 0.2126) * cr) * 255.0;
            let b = (yy + 2.0 * (1.0 - 0.0722) * cb) * 255.0;
            let i = (y * w as usize + x) * 4;
            // ramps change by 8 per pixel in one direction: the chroma average blurs by up to 4, which the inverse amplifies by up to 1.6
            assert!((r - rgba[i] as f64).abs() <= 10.0, "red at ({x},{y}): {r} vs {}", rgba[i]);
            assert!((b - rgba[i + 2] as f64).abs() <= 10.0, "blue at ({x},{y}): {b} vs {}", rgba[i + 2]);
        }
    }
}

#[test]
fn odd_sizes_repeat_the_last_row_and_column() {
    let (w, h) = (5u32, 3u32);
    let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i * 17) as u8, (i * 31) as u8, (i * 7) as u8, 255]).collect();
    let f = from_rgba8(&rgba, w, h, 8, false);
    assert_eq!((f.planes[0].len(), f.planes[1].len(), f.planes[2].len()), (15, 3 * 2, 3 * 2));
    assert_eq!(f.stride, [5, 3, 3]);
    // the corner chroma sample averages the one real pixel with its repeats: equal to its own value
    let last = (h * w - 1) as usize * 4;
    let solo = from_rgba8(&solid(2, 2, [rgba[last], rgba[last + 1], rgba[last + 2]]), 2, 2, 8, false);
    assert_eq!(f.planes[1][5], solo.planes[1][0]);
    assert_eq!(f.planes[2][5], solo.planes[2][0]);
    // a 1x1 picture works
    let one = from_rgba8(&[10, 20, 30, 255], 1, 1, 8, false);
    assert_eq!((one.planes[0].len(), one.planes[1].len()), (1, 1));
}

#[test]
fn sixteen_bit_input_matches_eight_bit_input_scaled_up() {
    let mut rng = Rng(3);
    let (w, h) = (16u32, 8u32);
    let rgba8: Vec<u8> = (0..w * h * 4).map(|_| rng.next() as u8).collect();
    let rgba16: Vec<u16> = rgba8.iter().map(|&v| v as u16 * 257).collect();
    for (bd, full) in [(8u8, false), (10, false), (8, true)] {
        let a = from_rgba8(&rgba8, w, h, bd, full);
        let b = from_rgba16(&rgba16, w, h, bd, full);
        assert_eq!(a, b, "bd{bd} full {full}");
    }
    // 16-bit input keeps the precision 8-bit input does not have
    let lo = from_rgba16(&[256 * 100 + 3, 0, 0, 65535], 1, 1, 10, true);
    let hi = from_rgba16(&[256 * 100 + 200, 0, 0, 65535], 1, 1, 10, true);
    assert!(hi.planes[0][0] >= lo.planes[0][0]);
}

#[test]
fn alpha_is_ignored() {
    let a = from_rgba8(&[90, 100, 110, 0], 1, 1, 8, false);
    let b = from_rgba8(&[90, 100, 110, 255], 1, 1, 8, false);
    assert_eq!(a, b);
}

#[test]
#[should_panic(expected = "width * height * 4")]
fn wrong_buffer_length_is_rejected() {
    from_rgba8(&[0; 12], 2, 2, 8, false);
}
