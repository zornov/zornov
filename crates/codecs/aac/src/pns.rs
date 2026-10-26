// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Perceptual noise substitution (ISO/IEC 14496-3 4.6.13).
//!
//! The standard leaves the noise generator open; this decoder uses the
//! 32-bit linear congruential generator `x' = 1664525 x + 1013904223` of the
//! ISO reference implementation family. Each noise band is filled with
//! uniformly distributed values and scaled so that its energy equals
//! `2^(0.5 * noise_nrg)` (the band's noise energy step is 1.5 dB).

use crate::ics::{ChannelFrame, BAND_STRIDE, NOISE_HCB};

#[derive(Clone)]
pub(crate) struct NoiseGen {
    state: u32,
}

impl NoiseGen {
    pub fn new() -> NoiseGen {
        NoiseGen { state: 0x1f2e3d4c }
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        self.state = self.state.wrapping_mul(1664525).wrapping_add(1013904223);
        self.state as i32 as f32
    }
}

/// Scale `band` so that its energy is `2^(0.5 * nrg)`.
pub(crate) fn scale_band(band: &mut [f32], nrg: i32) {
    let energy: f32 = band.iter().map(|v| v * v).sum();
    if energy <= 0.0 {
        return;
    }
    let target = 2f32.powf(0.25 * nrg as f32);
    let s = target / energy.sqrt();
    for v in band {
        *v *= s;
    }
}

/// Fill every noise band of `ch` with scaled random values.
pub(crate) fn fill(ch: &mut ChannelFrame, rng: &mut NoiseGen) {
    let ics = &ch.ics;
    let mut win = 0;
    for g in 0..ics.num_window_groups {
        for sfb in 0..ics.max_sfb {
            let idx = g * BAND_STRIDE + sfb;
            if ch.band_type[idx] != NOISE_HCB {
                continue;
            }
            let lo = ics.swb_offset[sfb] as usize;
            let hi = ics.swb_offset[sfb + 1] as usize;
            for w in 0..ics.group_len[g] {
                let band = &mut ch.spec[(win + w) * 128 + lo..(win + w) * 128 + hi];
                for v in band.iter_mut() {
                    *v = rng.next();
                }
                scale_band(band, ch.sf[idx]);
            }
        }
        win += ics.group_len[g];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_energy_matches_noise_energy() {
        let mut rng = NoiseGen::new();
        let mut band: Vec<f32> = (0..16).map(|_| rng.next()).collect();
        scale_band(&mut band, 20);
        let e: f32 = band.iter().map(|v| v * v).sum();
        assert!((e - 2f32.powf(10.0)).abs() / e < 1e-5);
        // Generator is not degenerate.
        let a = rng.next();
        let b = rng.next();
        assert_ne!(a, b);
    }
}
