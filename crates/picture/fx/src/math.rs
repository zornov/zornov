// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Small numeric helpers shared by the effects: hashing, procedural noise,
//! smooth steps and colour conversions.

use std::f32::consts::PI;

pub const TAU: f32 = 2.0 * PI;

#[inline]
pub fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Hermite step between `e0` and `e1`. When the edges coincide it acts as
/// a hard step instead of dividing by zero.
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (e1 - e0).abs() < 1e-9 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

/// Integer hash (a variant of the "lowbias32" mixer); good avalanche,
/// cheap, identical on CPU and GPU.
#[inline]
pub fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Hash of a lattice point to 0..1.
#[inline]
pub fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let h = hash_u32((x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ hash_u32(seed));
    (h >> 8) as f32 / (1u32 << 24) as f32
}

#[inline]
pub fn hash3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    hash2(x, y ^ (z.wrapping_mul(0x2c1b_3c6d_u32 as i32)), seed.wrapping_add(z as u32))
}

/// Hash of a single integer to 0..1.
#[inline]
pub fn hash1(x: i32, seed: u32) -> f32 {
    hash2(x, 0x51ed_270b, seed)
}

/// Smooth value noise in 0..1 on a unit lattice.
pub fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xf, yf) = (x.floor(), y.floor());
    let (xi, yi) = (xf as i32, yf as i32);
    let (tx, ty) = (fade(x - xf), fade(y - yf));
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    lerp(lerp(a, b, tx), lerp(c, d, tx), ty)
}

/// Value noise that also evolves smoothly along a third axis (time).
pub fn value_noise3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let zf = z.floor();
    let zi = zf as i32;
    let t = fade(z - zf);
    let a = value_noise(x, y, seed.wrapping_add((zi as u32).wrapping_mul(0x9e37_79b9)));
    let b = value_noise(x, y, seed.wrapping_add(((zi + 1) as u32).wrapping_mul(0x9e37_79b9)));
    lerp(a, b, t)
}

/// Fractal sum of value noise, normalised to 0..1.
pub fn fbm(x: f32, y: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut norm = 0.0;
    let (mut fx, mut fy, mut fz) = (x, y, z);
    for o in 0..octaves.max(1) {
        sum += amp * value_noise3(fx, fy, fz, seed.wrapping_add(o * 101));
        norm += amp;
        amp *= 0.5;
        // Rotate a little between octaves so lattice axes do not line up.
        let (rx, ry) = (fx * 1.6 - fy * 1.2, fx * 1.2 + fy * 1.6);
        fx = rx + 17.3;
        fy = ry + 5.1;
        fz *= 1.7;
    }
    sum / norm
}

/// Smooth 1-D noise in -1..1, for camera shake and wobbles.
pub fn noise1(t: f32, seed: u32) -> f32 {
    let f = t.floor();
    let i = f as i32;
    let u = fade(t - f);
    lerp(hash1(i, seed), hash1(i + 1, seed), u) * 2.0 - 1.0
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Hue (degrees), saturation and value to linear-ish RGB. Used for tints,
/// so the exact colour space does not matter much.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// A tiny deterministic random generator (xorshift32) for algorithms that
/// need a sequence, such as RANSAC.
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(hash_u32(seed) | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// 0..1
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u32() as usize) % n.max(1)
    }
}

/// Rotates a 2-D vector by `angle` radians.
#[inline]
pub fn rotate(v: [f32; 2], angle: f32) -> [f32; 2] {
    let (s, c) = angle.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_ranges() {
        for i in 0..500 {
            let x = i as f32 * 0.37;
            let v = fbm(x, x * 0.5, x * 0.1, 4, 7);
            assert!((0.0..=1.0).contains(&v));
            assert!((-1.0..=1.0).contains(&noise1(x, 3)));
        }
    }

    #[test]
    fn smoothstep_degenerate() {
        assert_eq!(smoothstep(0.5, 0.5, 0.4), 0.0);
        assert_eq!(smoothstep(0.5, 0.5, 0.6), 1.0);
    }
}
