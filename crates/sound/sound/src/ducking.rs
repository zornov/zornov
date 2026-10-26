// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ducking: lower music lanes while dialogue lanes carry speech.
//!
//! The [`Vad`](crate::vad::Vad) on the dialogue lanes decides when speech is
//! present; the [`Ducker`] turns that decision into a smooth gain ramp with
//! attack (how fast the music gets out of the way), hold (so the music does
//! not pump between words) and release (how gently it comes back).

use crate::buffer::db_to_gain;

/// Ducking settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuckingParams {
    /// How far the music drops while speech is present, in dB (0..40).
    pub amount_db: f32,
    /// Time to reach the ducked level, in ms (10..1000).
    pub attack_ms: f32,
    /// Time the music stays ducked after speech ends, in ms (0..2000).
    pub hold_ms: f32,
    /// Time to return to full level, in ms (50..5000).
    pub release_ms: f32,
}

impl Default for DuckingParams {
    fn default() -> Self {
        Self { amount_db: 12.0, attack_ms: 120.0, hold_ms: 400.0, release_ms: 800.0 }
    }
}

/// Turns a speech decision into a per-sample gain ramp.
pub struct Ducker {
    rate: u32,
    params: DuckingParams,
    gain_db: f32,
    hold_left: usize,
}

impl Ducker {
    /// A ducker at `rate`.
    pub fn new(rate: u32, params: DuckingParams) -> Self {
        Self { rate, params, gain_db: 0.0, hold_left: 0 }
    }

    /// Applies new settings.
    pub fn set_params(&mut self, p: DuckingParams) {
        self.params = p;
    }

    /// Current settings.
    pub fn params(&self) -> &DuckingParams {
        &self.params
    }

    /// Fills `gains` with linear gains for the next block given whether
    /// speech is present during it. The ramp is linear in dB, which sounds
    /// even to the ear.
    pub fn fill(&mut self, speech: bool, gains: &mut [f32]) {
        let p = self.params;
        let target = -p.amount_db.clamp(0.0, 40.0);
        let attack_step = p.amount_db / (p.attack_ms.max(1.0) * 0.001 * self.rate as f32);
        let release_step = p.amount_db / (p.release_ms.max(1.0) * 0.001 * self.rate as f32);
        let hold_frames = (p.hold_ms.max(0.0) * 0.001 * self.rate as f32) as usize;
        for g in gains.iter_mut() {
            if speech {
                self.hold_left = hold_frames;
                self.gain_db = (self.gain_db - attack_step).max(target);
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.gain_db = (self.gain_db + release_step).min(0.0);
            }
            *g = db_to_gain(self.gain_db);
        }
    }

    /// Current gain in dB (0 when not ducking).
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// Back to full level immediately (on seek).
    pub fn reset(&mut self) {
        self.gain_db = 0.0;
        self.hold_left = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ducks_holds_and_releases_with_the_configured_times() {
        let rate = 48_000;
        let p = DuckingParams { amount_db: 12.0, attack_ms: 100.0, hold_ms: 200.0, release_ms: 400.0 };
        let mut d = Ducker::new(rate, p);
        let mut g = vec![0.0; 4800];
        d.fill(true, &mut g);
        assert!((g[2399] - db_to_gain(-6.0)).abs() < 0.01, "half way: {}", g[2399]);
        assert!((g[4799] - db_to_gain(-12.0)).abs() < 1e-3);
        d.fill(true, &mut g);
        assert!((d.gain_db() + 12.0).abs() < 1e-4, "stays ducked");
        // Hold: 200 ms at the ducked level after speech stops.
        let mut h = vec![0.0; 9600];
        d.fill(false, &mut h);
        assert!((h[9500] - db_to_gain(-12.0)).abs() < 1e-3, "{}", h[9500]);
        // Release over 400 ms.
        let mut r = vec![0.0; 19_200];
        d.fill(false, &mut r);
        assert!((r[9599] - db_to_gain(-6.0)).abs() < 0.01, "{}", r[9599]);
        assert!((r[19_199] - 1.0).abs() < 1e-3);
    }
}
