// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Small deterministic noise and random numbers for building worlds.

/// A fast seeded random stream (xorshift32).
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng(seed.wrapping_mul(2_654_435_761).wrapping_add(0x9E37_79B9) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    /// 0..1
    pub fn next(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
    pub fn index(&mut self, n: usize) -> usize {
        (self.next_u32() as usize) % n.max(1)
    }
}

fn hash2(x: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x85EB_CA6B) ^ (z as u32).wrapping_mul(0xC2B2_AE35) ^ seed.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / 16_777_216.0
}

/// Smooth value noise, -1..1.
pub fn value(x: f32, z: f32, seed: u32) -> f32 {
    let (xi, zi) = (x.floor(), z.floor());
    let (fx, fz) = (x - xi, z - zi);
    let (xi, zi) = (xi as i32, zi as i32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (u, v) = (s(fx), s(fz));
    let a = hash2(xi, zi, seed);
    let b = hash2(xi + 1, zi, seed);
    let c = hash2(xi, zi + 1, seed);
    let d = hash2(xi + 1, zi + 1, seed);
    let top = a + (b - a) * u;
    let bot = c + (d - c) * u;
    (top + (bot - top) * v) * 2.0 - 1.0
}

/// Fractal noise, roughly -1..1.
pub fn fbm(x: f32, z: f32, octaves: usize, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += amp * value(x * freq, z * freq, seed.wrapping_add(o as u32 * 101));
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm.max(1e-6)
}

/// Ridged noise, 0..1: sharp crests.
pub fn ridged(x: f32, z: f32, octaves: usize, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        let r = 1.0 - value(x * freq, z * freq, seed.wrapping_add(o as u32 * 57)).abs();
        sum += amp * r * r;
        norm += amp;
        amp *= 0.5;
        freq *= 2.1;
    }
    sum / norm.max(1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_bounded_and_varies() {
        let a = fbm(3.3, 7.1, 5, 4);
        assert_eq!(a, fbm(3.3, 7.1, 5, 4));
        assert_ne!(a, fbm(3.3, 7.1, 5, 5));
        let mut lo = 9.0f32;
        let mut hi = -9.0f32;
        for i in 0..400 {
            let v = fbm(i as f32 * 0.37, i as f32 * 0.11, 4, 1);
            lo = lo.min(v);
            hi = hi.max(v);
            assert!((0.0..=1.0).contains(&ridged(i as f32 * 0.2, 1.0, 3, 2)));
        }
        assert!(lo < -0.2 && hi > 0.2 && lo >= -1.0 && hi <= 1.0);
        let mut r = Rng::new(1);
        let s: f32 = (0..1000).map(|_| r.next()).sum::<f32>() / 1000.0;
        assert!((s - 0.5).abs() < 0.05);
    }
}
