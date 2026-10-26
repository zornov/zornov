// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stereo delay (echo) with feedback, ping-pong, and a tone filter in the
//! feedback loop. Delay times move smoothly (the read head slews towards a
//! new time) so automation does not click.

use crate::buffer::Effect;
use crate::eq::{BandKind, Biquad, Coeffs};

/// Delay settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayParams {
    /// Left delay time in ms (1..4000).
    pub time_left_ms: f32,
    /// Right delay time in ms (1..4000).
    pub time_right_ms: f32,
    /// Feedback amount (0..0.95).
    pub feedback: f32,
    /// Cross-feed the channels so echoes bounce left-right.
    pub ping_pong: bool,
    /// Low-pass in the feedback loop in Hz (each repeat gets darker).
    pub high_cut_hz: f32,
    /// High-pass in the feedback loop in Hz (keeps repeats from booming).
    pub low_cut_hz: f32,
    /// Wet/dry balance (0..1).
    pub mix: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self { time_left_ms: 375.0, time_right_ms: 375.0, feedback: 0.35, ping_pong: false, high_cut_hz: 6000.0, low_cut_hz: 150.0, mix: 0.3 }
    }
}

impl DelayParams {
    /// Delay times for a tempo: `beats` (0.25 = sixteenth, 1 = quarter) at `bpm`.
    pub fn synced(bpm: f32, beats: f32) -> Self {
        let ms = 60_000.0 / bpm.max(20.0) * beats;
        Self { time_left_ms: ms, time_right_ms: ms, ..Default::default() }
    }
}

const MAX_DELAY_S: f32 = 4.0;

/// The stereo delay.
pub struct StereoDelay {
    rate: u32,
    params: DelayParams,
    buf: [Vec<f32>; 2],
    write: usize,
    time: [f32; 2],
    lp: Biquad,
    hp: Biquad,
}

impl StereoDelay {
    /// Builds a delay at `rate` with up to 4 s of time.
    pub fn new(rate: u32, params: DelayParams) -> Self {
        let cap = (MAX_DELAY_S * rate as f32) as usize + 2;
        let mut d = Self { rate, params, buf: [vec![0.0; cap], vec![0.0; cap]], write: 0, time: [0.0; 2], lp: Biquad::default(), hp: Biquad::default() };
        d.set_params(params);
        d.time = d.targets();
        d
    }

    fn targets(&self) -> [f32; 2] {
        let max = (self.buf[0].len() - 2) as f32;
        let f = |ms: f32| (ms.clamp(1.0, MAX_DELAY_S * 1000.0) * 0.001 * self.rate as f32).min(max);
        [f(self.params.time_left_ms), f(self.params.time_right_ms)]
    }

    /// Applies new settings.
    pub fn set_params(&mut self, p: DelayParams) {
        self.params = p;
        self.lp.coeffs = Coeffs::design(BandKind::LowPass, self.rate, p.high_cut_hz.clamp(200.0, 20_000.0), 0.707, 0.0);
        self.hp.coeffs = Coeffs::design(BandKind::HighPass, self.rate, p.low_cut_hz.clamp(20.0, 2000.0), 0.707, 0.0);
    }

    /// Current settings.
    pub fn params(&self) -> &DelayParams {
        &self.params
    }

    #[inline]
    fn read(&self, ch: usize, delay: f32) -> f32 {
        let n = self.buf[ch].len();
        let d = delay.max(1.0);
        let i = d as usize;
        let f = d - i as f32;
        let a = self.buf[ch][(self.write + n - i) % n];
        let b = self.buf[ch][(self.write + n - i - 1) % n];
        a + (b - a) * f
    }
}

impl Effect for StereoDelay {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let targets = self.targets();
        let fb = self.params.feedback.clamp(0.0, 0.95);
        let mix = self.params.mix.clamp(0.0, 1.0);
        let n = self.buf[0].len();
        // Slew ~ 1 ms per sample of time change at most: a glide, not a jump.
        let slew = 0.002 * self.rate as f32 / 1000.0 + 0.5;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            for ch in 0..2 {
                let d = targets[ch] - self.time[ch];
                self.time[ch] += d.clamp(-slew, slew);
            }
            let wet_l = self.read(0, self.time[0]);
            let wet_r = self.read(1, self.time[1]);
            let fb_l = self.hp.tick(0, self.lp.tick(0, wet_l)) * fb;
            let fb_r = self.hp.tick(1, self.lp.tick(1, wet_r)) * fb;
            let (in_l, in_r) = if self.params.ping_pong { (*l + fb_r, fb_l) } else { (*l + fb_l, *r + fb_r) };
            self.buf[0][self.write] = in_l;
            self.buf[1][self.write] = in_r;
            self.write = (self.write + 1) % n;
            *l += (wet_l - *l) * mix;
            *r += (wet_r - *r) * mix;
        }
    }

    fn reset(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
        self.write = 0;
        self.lp.reset();
        self.hp.reset();
        self.time = self.targets();
    }

    fn tail(&self) -> usize {
        let fb = self.params.feedback.clamp(0.0, 0.95);
        let repeats = if fb < 1e-3 { 1.0 } else { (60.0 / (-20.0 * fb.log10())).ceil() };
        (self.time[0].max(self.time[1]) * (repeats + 1.0)) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 20 ms 1 kHz burst at the start of `n` frames.
    fn burst(n: usize) -> Vec<f32> {
        (0..n).map(|i| if i < 960 { (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin() } else { 0.0 }).collect()
    }

    fn peak_near(x: &[f32], t: usize) -> f32 {
        x[t..t + 960].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn echoes_land_at_the_delay_time_and_decay_by_feedback() {
        let rate = 48_000;
        let p = DelayParams { time_left_ms: 100.0, time_right_ms: 100.0, feedback: 0.5, ping_pong: false, high_cut_hz: 20_000.0, low_cut_hz: 20.0, mix: 1.0 };
        let mut d = StereoDelay::new(rate, p);
        let mut l = burst(48_000);
        let mut r = l.clone();
        d.process(&mut l, &mut r);
        assert!((peak_near(&l, 4800) - 1.0).abs() < 0.03, "{}", peak_near(&l, 4800));
        assert!((peak_near(&l, 9600) - 0.5).abs() < 0.03, "{}", peak_near(&l, 9600));
        assert!((peak_near(&l, 14_400) - 0.25).abs() < 0.03, "{}", peak_near(&l, 14_400));
        assert!(l[1000..4790].iter().all(|v| v.abs() < 1e-3));
        assert!(d.tail() > 9600);
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let rate = 48_000;
        let p = DelayParams { time_left_ms: 50.0, time_right_ms: 50.0, feedback: 0.6, ping_pong: true, high_cut_hz: 20_000.0, low_cut_hz: 20.0, mix: 1.0 };
        let mut d = StereoDelay::new(rate, p);
        let mut l = burst(12_000);
        let mut r = vec![0.0; 12_000];
        d.process(&mut l, &mut r);
        assert!(peak_near(&l, 2400) > 0.95 && peak_near(&r, 2400) < 0.01);
        assert!((peak_near(&r, 4800) - 0.6).abs() < 0.03 && peak_near(&l, 4800) < 0.01);
        assert!((peak_near(&l, 7200) - 0.36).abs() < 0.03 && peak_near(&r, 7200) < 0.01);
    }
}
