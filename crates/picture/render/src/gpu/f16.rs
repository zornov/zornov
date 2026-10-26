// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! IEEE 754 half-precision conversions for moving `f32` frames to and from
//! `rgba16float` textures. Round-to-nearest-even, with subnormals, so a
//! value survives an upload and readback as the nearest representable half.

/// `f32` → half bits.
#[inline]
pub fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 0xff {
        // Infinity or NaN; keep a NaN payload bit so NaN stays NaN.
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        // Subnormal half: shift the (implicit-1) mantissa into place.
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        let half_m = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let round_up = rem > halfway || (rem == halfway && (half_m & 1) == 1);
        return sign | (half_m as u16 + round_up as u16);
    }
    let half = ((e as u32) << 10) | (mant >> 13);
    let rem = mant & 0x1fff;
    let round_up = rem > 0x1000 || (rem == 0x1000 && (half & 1) == 1);
    // Rounding may carry into the exponent, which is the correct result
    // (and produces infinity from the largest finite values).
    sign | (half + round_up as u32) as u16
}

/// Half bits → `f32`.
#[inline]
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            // Subnormal: normalise.
            let mut m = mant;
            let mut e: i32 = 1;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x3ff;
            sign | (((e + 127 - 15) as u32) << 23) | (m << 13)
        }
    } else if exp == 0x1f {
        sign | 0x7f80_0000 | (mant << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

/// Packs premultiplied `f32` pixels as little-endian half RGBA bytes.
pub fn pixels_to_f16_bytes(pixels: &[[f32; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * 8);
    for p in pixels {
        for c in p {
            out.extend_from_slice(&f32_to_f16(*c).to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_representable_values() {
        for v in [0.0f32, 1.0, -1.0, 0.5, 0.25, 2.0, 1024.0, 65504.0, 2f32.powi(-14), 2f32.powi(-24), 3.0 * 2f32.powi(-20), -0.125] {
            let h = f32_to_f16(v);
            assert_eq!(f16_to_f32(h), v, "{v}");
        }
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(-2.0), 0xc000);
        assert!(f16_to_f32(f32_to_f16(f32::INFINITY)).is_infinite());
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
        assert!(f16_to_f32(f32_to_f16(1e6)).is_infinite());
    }

    #[test]
    fn rounding_is_nearest_even() {
        // 1 + 2^-11 is exactly halfway between 1.0 and the next half; ties
        // go to the even mantissa (1.0).
        assert_eq!(f32_to_f16(1.0 + 2f32.powi(-11)), 0x3c00);
        // 1 + 3 * 2^-11 is halfway between the first and second steps; the
        // even one is the second.
        assert_eq!(f32_to_f16(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        // Relative error of a random value is within half an ulp (2^-11).
        for i in 1..2000 {
            let v = i as f32 * 0.0137;
            let r = f16_to_f32(f32_to_f16(v));
            assert!(((r - v) / v).abs() <= 2f32.powi(-11), "{v} -> {r}");
        }
    }
}
