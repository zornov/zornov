// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! IEEE 754 binary16 ("half") conversion, as OpenEXR stores it.

/// Half bits → f32 (exact).
pub fn to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1F) as u32;
    let man = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if man == 0 {
            sign
        } else {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 0x1F {
        sign | 0x7F80_0000 | (man << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

/// f32 → half bits, rounding to nearest even; overflow becomes infinity.
pub fn from_f32(f: f32) -> u16 {
    let x = f.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xFF) as i32;
    let man = x & 0x7F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1F {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rest = m & ((1 << shift) - 1);
        let mut r = m >> shift;
        if rest > half || (rest == half && r & 1 == 1) {
            r += 1;
        }
        return sign | r as u16;
    }
    let mut r = ((e as u32) << 10) | (man >> 13);
    let rest = man & 0x1FFF;
    if rest > 0x1000 || (rest == 0x1000 && r & 1 == 1) {
        r += 1;
    }
    sign | r as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_values_round_trip() {
        for h in 0..=0xFFFFu16 {
            let f = to_f32(h);
            if f.is_nan() {
                continue;
            }
            assert_eq!(from_f32(f), h, "{h:#x} -> {f}");
        }
        assert_eq!(to_f32(0x3C00), 1.0);
        assert_eq!(to_f32(0xC000), -2.0);
        assert_eq!(from_f32(65504.0), 0x7BFF);
        assert_eq!(from_f32(1e6), 0x7C00);
        assert_eq!(from_f32(0.18), 0x31C3);
    }
}
