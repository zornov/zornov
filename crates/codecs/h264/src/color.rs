// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! YCbCr -> RGBA conversion using the VUI colour description.

use crate::decoder::{ColorInfo, Frame};

/// Luma coefficients (Kr, Kb) for a matrix_coefficients value.
fn coefficients(c: &ColorInfo, height: u32) -> (f64, f64) {
    match c.matrix_coefficients {
        1 => (0.2126, 0.0722),
        4 => (0.30, 0.11),
        5 | 6 => (0.299, 0.114),
        7 => (0.212, 0.087),
        9 | 10 => (0.2627, 0.0593),
        // Unspecified / reserved: HD sizes are BT.709, SD BT.601.
        _ => {
            if height > 576 {
                (0.2126, 0.0722)
            } else {
                (0.299, 0.114)
            }
        }
    }
}

/// Fixed-point conversion parameters (16.16).
struct Params {
    y_off: i32,
    y_mul: i32,
    cr_r: i32,
    cb_g: i32,
    cr_g: i32,
    cb_b: i32,
}

fn params(frame: &Frame) -> Params {
    let (kr, kb) = coefficients(&frame.color, frame.height);
    let kg = 1.0 - kr - kb;
    let (y_off, y_scale, c_scale) = if frame.color.full_range { (0, 1.0, 1.0) } else { (16, 255.0 / 219.0, 255.0 / 224.0) };
    let f = |v: f64| (v * 65536.0).round() as i32;
    Params {
        y_off,
        y_mul: f(y_scale),
        cr_r: f(2.0 * (1.0 - kr) * c_scale),
        cb_g: f(2.0 * kb * (1.0 - kb) / kg * c_scale),
        cr_g: f(2.0 * kr * (1.0 - kr) / kg * c_scale),
        cb_b: f(2.0 * (1.0 - kb) * c_scale),
    }
}

/// Convert a decoded frame to packed 8-bit RGBA (alpha = 255), `width * height * 4` bytes.
/// Uses BT.601/709/2020 coefficients and limited/full range as signalled in the VUI.
pub fn to_rgba(frame: &Frame) -> Vec<u8> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let cw = frame.chroma_width as usize;
    let p = params(frame);
    let mut out = vec![0u8; w * h * 4];
    let clip = |v: i32| -> u8 { ((v + 32768) >> 16).clamp(0, 255) as u8 };
    for y in 0..h {
        let yrow = &frame.y[y * w..y * w + w];
        let crow = (y / 2) * cw;
        let o = &mut out[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            let yy = (yrow[x] as i32 - p.y_off) * p.y_mul;
            let cb = frame.u[crow + x / 2] as i32 - 128;
            let cr = frame.v[crow + x / 2] as i32 - 128;
            o[x * 4] = clip(yy + p.cr_r * cr);
            o[x * 4 + 1] = clip(yy - p.cb_g * cb - p.cr_g * cr);
            o[x * 4 + 2] = clip(yy + p.cb_b * cb);
            o[x * 4 + 3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(yv: u8, u: u8, v: u8, full: bool, mc: u8) -> Frame {
        Frame {
            width: 2,
            height: 2,
            chroma_width: 1,
            chroma_height: 1,
            y: vec![yv; 4],
            u: vec![u],
            v: vec![v],
            poc: 0,
            pts: None,
            idr: true,
            interlaced: false,
            color: ColorInfo { matrix_coefficients: mc, colour_primaries: 2, transfer_characteristics: 2, full_range: full },
            sar: (1, 1),
        }
    }

    #[test]
    fn limited_range_black_white() {
        assert_eq!(&to_rgba(&frame(16, 128, 128, false, 1))[..4], &[0, 0, 0, 255]);
        assert_eq!(&to_rgba(&frame(235, 128, 128, false, 1))[..4], &[255, 255, 255, 255]);
        assert_eq!(&to_rgba(&frame(128, 128, 128, true, 6))[..4], &[128, 128, 128, 255]);
    }

    #[test]
    fn bt601_red() {
        // Limited range BT.601 red: Y=81, Cb=90, Cr=240.
        let px = to_rgba(&frame(81, 90, 240, false, 6));
        assert!(px[0] >= 253 && px[1] <= 2 && px[2] <= 2, "{px:?}");
    }
}
