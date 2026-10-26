// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Algorithmic plate reverb.
//!
//! The structure is the one Jon Dattorro published in "Effect Design, Part 1:
//! Reverberator and Other Filters" (JAES 45(9), 1997): a band-limited input
//! runs through four all-pass diffusers into a figure-of-eight tank of two
//! branches, each a modulated all-pass, a delay, a damping low-pass, a second
//! all-pass and a second delay, cross-coupled with a decay gain. Output taps
//! read from several points of both branches so the stereo image is wide
//! and uncorrelated. Delay lengths scale with the sample rate (the paper's
//! are for 29.761 kHz) and with the `size` control, within buffers sized
//! for the largest size at construction so changes never allocate.

use crate::buffer::Effect;
use crate::eq::{BandKind, Biquad, Coeffs};

/// Reverb settings. All in 0..1 unless noted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    /// Delay before the reverb starts, in ms (0..200).
    pub pre_delay_ms: f32,
    /// Room scale (0.3 small booth .. 2.0 cathedral); scales every delay.
    pub size: f32,
    /// Tank feedback (0 short .. 1 near-infinite).
    pub decay: f32,
    /// High-frequency loss inside the tank.
    pub damping: f32,
    /// Input diffusion strength (lower = more distinct early echoes).
    pub diffusion: f32,
    /// High-pass on the wet signal in Hz (20..1000).
    pub low_cut_hz: f32,
    /// Low-pass on the wet signal in Hz (1000..20000).
    pub high_cut_hz: f32,
    /// Stereo width of the wet signal (0 mono .. 1 full).
    pub width: f32,
    /// Wet/dry balance (0 dry .. 1 wet).
    pub mix: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self { pre_delay_ms: 10.0, size: 1.0, decay: 0.5, damping: 0.3, diffusion: 0.7, low_cut_hz: 100.0, high_cut_hz: 12_000.0, width: 1.0, mix: 0.25 }
    }
}

impl ReverbParams {
    /// A medium concert hall.
    pub const HALL: Self = Self { pre_delay_ms: 25.0, size: 1.6, decay: 0.78, damping: 0.35, diffusion: 0.75, low_cut_hz: 120.0, high_cut_hz: 9000.0, width: 1.0, mix: 0.3 };
    /// A tight vocal plate.
    pub const PLATE: Self = Self { pre_delay_ms: 8.0, size: 0.9, decay: 0.55, damping: 0.2, diffusion: 0.8, low_cut_hz: 150.0, high_cut_hz: 14_000.0, width: 1.0, mix: 0.2 };
    /// A small bright room.
    pub const ROOM: Self = Self { pre_delay_ms: 4.0, size: 0.5, decay: 0.3, damping: 0.4, diffusion: 0.6, low_cut_hz: 100.0, high_cut_hz: 12_000.0, width: 0.8, mix: 0.18 };
}

/// Room presets for dialogue and effects placement, as `(id, label, params)`:
/// the spaces a film mixer puts a line or a sound into. `mix` is a starting
/// point for a clip insert; on an aux return set it to 1.
pub const ROOMS: [(&str, &str, ReverbParams); 12] = [
    ("booth", "Booth (nearly dry)", ReverbParams { pre_delay_ms: 1.0, size: 0.3, decay: 0.12, damping: 0.6, diffusion: 0.5, low_cut_hz: 200.0, high_cut_hz: 9000.0, width: 0.5, mix: 0.08 }),
    ("car", "Car interior", ReverbParams { pre_delay_ms: 1.0, size: 0.3, decay: 0.15, damping: 0.75, diffusion: 0.4, low_cut_hz: 150.0, high_cut_hz: 6000.0, width: 0.4, mix: 0.12 }),
    ("small_room", "Small room", ReverbParams { pre_delay_ms: 3.0, size: 0.45, decay: 0.28, damping: 0.45, diffusion: 0.6, low_cut_hz: 120.0, high_cut_hz: 10_000.0, width: 0.7, mix: 0.15 }),
    ("living_room", "Living room", ReverbParams { pre_delay_ms: 6.0, size: 0.6, decay: 0.35, damping: 0.55, diffusion: 0.65, low_cut_hz: 110.0, high_cut_hz: 9000.0, width: 0.8, mix: 0.16 }),
    ("bathroom", "Bathroom (tiled)", ReverbParams { pre_delay_ms: 2.0, size: 0.4, decay: 0.55, damping: 0.1, diffusion: 0.8, low_cut_hz: 150.0, high_cut_hz: 14_000.0, width: 0.7, mix: 0.25 }),
    ("office", "Office", ReverbParams { pre_delay_ms: 5.0, size: 0.7, decay: 0.32, damping: 0.6, diffusion: 0.6, low_cut_hz: 120.0, high_cut_hz: 8000.0, width: 0.8, mix: 0.14 }),
    ("corridor", "Corridor", ReverbParams { pre_delay_ms: 12.0, size: 0.9, decay: 0.5, damping: 0.3, diffusion: 0.45, low_cut_hz: 150.0, high_cut_hz: 11_000.0, width: 0.6, mix: 0.2 }),
    ("hall", "Concert hall", ReverbParams::HALL),
    ("church", "Church / cathedral", ReverbParams { pre_delay_ms: 40.0, size: 2.0, decay: 0.9, damping: 0.4, diffusion: 0.8, low_cut_hz: 120.0, high_cut_hz: 7000.0, width: 1.0, mix: 0.32 }),
    ("warehouse", "Warehouse", ReverbParams { pre_delay_ms: 30.0, size: 1.8, decay: 0.75, damping: 0.25, diffusion: 0.55, low_cut_hz: 100.0, high_cut_hz: 9000.0, width: 1.0, mix: 0.28 }),
    ("cave", "Cave / tunnel", ReverbParams { pre_delay_ms: 25.0, size: 1.5, decay: 0.82, damping: 0.5, diffusion: 0.4, low_cut_hz: 90.0, high_cut_hz: 6000.0, width: 1.0, mix: 0.3 }),
    ("outdoor", "Outdoors (slap off buildings)", ReverbParams { pre_delay_ms: 60.0, size: 1.2, decay: 0.2, damping: 0.7, diffusion: 0.2, low_cut_hz: 200.0, high_cut_hz: 7000.0, width: 1.0, mix: 0.1 }),
];

impl ReverbParams {
    /// A room preset by id (see [`ROOMS`]; `plate` and `room` too).
    pub fn room(id: &str) -> Option<ReverbParams> {
        let k = id.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        match k.as_str() {
            "plate" => return Some(ReverbParams::PLATE),
            "room" => return Some(ReverbParams::ROOM),
            "cathedral" => return Some(ROOMS[8].2),
            "tunnel" => return Some(ROOMS[10].2),
            "kitchen" | "tiled" => return Some(ROOMS[4].2),
            _ => {}
        }
        ROOMS.iter().find(|r| r.0 == k).map(|r| r.2)
    }
}

/// Reference sample rate of the published delay lengths.
const REF_RATE: f32 = 29_761.0;
const MAX_SIZE: f32 = 2.0;

/// A fixed-capacity delay line with fractional reads.
#[derive(Clone, Debug)]
struct Line {
    buf: Vec<f32>,
    write: usize,
}

impl Line {
    fn new(capacity: usize) -> Self {
        Self { buf: vec![0.0; capacity.max(2)], write: 0 }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.buf[self.write] = x;
        self.write = (self.write + 1) % self.buf.len();
    }

    /// Sample written `delay` frames ago (delay ≥ 1).
    #[inline]
    fn read(&self, delay: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.write + n - delay.clamp(1, n - 1)) % n]
    }

    /// Linear-interpolated read for a fractional delay.
    #[inline]
    fn read_frac(&self, delay: f32) -> f32 {
        let d = delay.max(1.0);
        let i = d as usize;
        let f = d - i as f32;
        self.read(i) + (self.read(i + 1) - self.read(i)) * f
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.write = 0;
    }
}

/// Schroeder all-pass built on a [`Line`].
#[derive(Clone, Debug)]
struct AllPass {
    line: Line,
    gain: f32,
}

impl AllPass {
    fn new(capacity: usize, gain: f32) -> Self {
        Self { line: Line::new(capacity), gain }
    }

    #[inline]
    fn tick(&mut self, x: f32, delay: f32) -> f32 {
        let d = self.line.read_frac(delay);
        let v = x - self.gain * d;
        self.line.push(v);
        d + self.gain * v
    }
}

/// The plate reverb.
pub struct Reverb {
    rate: u32,
    params: ReverbParams,
    scale: f32,
    pre: Line,
    bandwidth: Biquad,
    diffusers: [AllPass; 4],
    // Tank, two branches.
    mod_ap: [AllPass; 2],
    delay1: [Line; 2],
    damp: [f32; 2],
    ap2: [AllPass; 2],
    delay2: [Line; 2],
    lfo_phase: f32,
    low_cut: Biquad,
    high_cut: Biquad,
}

// Published lengths at REF_RATE: diffusers, modulated all-passes, delays 1,
// decay all-passes, delays 2, and the output tap offsets.
const DIFF: [f32; 4] = [142.0, 107.0, 379.0, 277.0];
const MOD_AP: [f32; 2] = [672.0, 908.0];
const DELAY1: [f32; 2] = [4453.0, 4217.0];
const AP2: [f32; 2] = [1800.0, 2656.0];
const DELAY2: [f32; 2] = [3720.0, 3163.0];
const EXCURSION: f32 = 16.0;
// Left output: + branchB.delay1[266] + branchB.delay1[2974] − branchB.ap2[1913]
//              + branchB.delay2[1996] − branchA.delay1[1990] − branchA.ap2[187] − branchA.delay2[1066]
const TAPS_L: [(usize, usize, f32); 7] = [(1, 0, 266.0), (1, 0, 2974.0), (1, 1, 1913.0), (1, 2, 1996.0), (0, 0, 1990.0), (0, 1, 187.0), (0, 2, 1066.0)];
const TAPS_R: [(usize, usize, f32); 7] = [(0, 0, 353.0), (0, 0, 3627.0), (0, 1, 1228.0), (0, 2, 2673.0), (1, 0, 2111.0), (1, 1, 335.0), (1, 2, 121.0)];
const TAP_SIGNS: [f32; 7] = [1.0, 1.0, -1.0, 1.0, -1.0, -1.0, -1.0];

impl Reverb {
    /// Builds the reverb at `rate` with room for `size` up to 2.0.
    pub fn new(rate: u32, params: ReverbParams) -> Self {
        let max_scale = rate as f32 / REF_RATE * MAX_SIZE;
        let cap = |len: f32| (len * max_scale) as usize + 4;
        let pre_cap = (0.2 * rate as f32) as usize + 2;
        let mut r = Self {
            rate,
            params,
            scale: 1.0,
            pre: Line::new(pre_cap),
            bandwidth: Biquad::default(),
            diffusers: [AllPass::new(cap(DIFF[0]), 0.75), AllPass::new(cap(DIFF[1]), 0.75), AllPass::new(cap(DIFF[2]), 0.625), AllPass::new(cap(DIFF[3]), 0.625)],
            mod_ap: [AllPass::new(cap(MOD_AP[0] + EXCURSION), -0.7), AllPass::new(cap(MOD_AP[1] + EXCURSION), -0.7)],
            delay1: [Line::new(cap(DELAY1[0])), Line::new(cap(DELAY1[1]))],
            damp: [0.0; 2],
            ap2: [AllPass::new(cap(AP2[0]), 0.5), AllPass::new(cap(AP2[1]), 0.5)],
            delay2: [Line::new(cap(DELAY2[0])), Line::new(cap(DELAY2[1]))],
            lfo_phase: 0.0,
            low_cut: Biquad::default(),
            high_cut: Biquad::default(),
        };
        r.set_params(params);
        r
    }

    /// Applies new settings (allocation-free).
    pub fn set_params(&mut self, p: ReverbParams) {
        self.params = p;
        self.scale = self.rate as f32 / REF_RATE * p.size.clamp(0.3, MAX_SIZE);
        let d = p.diffusion.clamp(0.0, 1.0);
        self.diffusers[0].gain = 0.75 * d;
        self.diffusers[1].gain = 0.75 * d;
        self.diffusers[2].gain = 0.625 * d;
        self.diffusers[3].gain = 0.625 * d;
        self.bandwidth.coeffs = Coeffs::design(BandKind::LowPass, self.rate, 14_000.0, 0.5, 0.0);
        self.low_cut.coeffs = Coeffs::design(BandKind::HighPass, self.rate, p.low_cut_hz.clamp(20.0, 2000.0), 0.707, 0.0);
        self.high_cut.coeffs = Coeffs::design(BandKind::LowPass, self.rate, p.high_cut_hz.clamp(1000.0, 20_000.0), 0.707, 0.0);
    }

    /// Current settings.
    pub fn params(&self) -> &ReverbParams {
        &self.params
    }
}

impl Effect for Reverb {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let p = self.params;
        let s = self.scale;
        let decay = p.decay.clamp(0.0, 0.98);
        let damping = p.damping.clamp(0.0, 1.0) * 0.9;
        let pre = (p.pre_delay_ms.clamp(0.0, 200.0) * 0.001 * self.rate as f32).max(1.0);
        let lfo_step = 0.5 / self.rate as f32; // 0.5 Hz modulation
        let mix = p.mix.clamp(0.0, 1.0);
        let width = p.width.clamp(0.0, 1.0);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let dry_l = *l;
            let dry_r = *r;
            // Input: mono sum, pre-delay, bandwidth, diffusion.
            self.pre.push(0.5 * (dry_l + dry_r));
            let mut x = self.pre.read_frac(pre);
            x = self.bandwidth.tick(0, x);
            for (i, ap) in self.diffusers.iter_mut().enumerate() {
                x = ap.tick(x, DIFF[i] * s);
            }
            // Tank. Each branch is fed by the input plus the other branch's end.
            self.lfo_phase = (self.lfo_phase + lfo_step) % 1.0;
            let lfo = (self.lfo_phase * std::f32::consts::TAU).sin() * EXCURSION * s;
            let mut ends = [0.0f32; 2];
            for b in 0..2 {
                ends[b] = self.delay2[b].read_frac(DELAY2[b] * s);
            }
            for b in 0..2 {
                let other = ends[1 - b];
                let m = if b == 0 { lfo } else { -lfo };
                let mut v = self.mod_ap[b].tick(x + other * decay, MOD_AP[b] * s + m);
                self.delay1[b].push(v);
                v = self.delay1[b].read_frac(DELAY1[b] * s);
                // One-pole damping low-pass.
                self.damp[b] = v + damping * (self.damp[b] - v);
                v = self.damp[b] * decay;
                v = self.ap2[b].tick(v, AP2[b] * s);
                self.delay2[b].push(v);
            }
            // Output taps.
            let mut wet_l = 0.0;
            let mut wet_r = 0.0;
            for k in 0..7 {
                let (b, which, off) = TAPS_L[k];
                wet_l += TAP_SIGNS[k] * self.tap(b, which, off * s);
                let (b, which, off) = TAPS_R[k];
                wet_r += TAP_SIGNS[k] * self.tap(b, which, off * s);
            }
            wet_l *= 0.6;
            wet_r *= 0.6;
            wet_l = self.high_cut.tick(0, self.low_cut.tick(0, wet_l));
            wet_r = self.high_cut.tick(1, self.low_cut.tick(1, wet_r));
            let mid = 0.5 * (wet_l + wet_r);
            let side = 0.5 * (wet_l - wet_r) * width;
            *l = dry_l + (mid + side - dry_l) * mix;
            *r = dry_r + (mid - side - dry_r) * mix;
        }
    }

    fn reset(&mut self) {
        self.pre.clear();
        self.bandwidth.reset();
        for ap in self.diffusers.iter_mut().chain(self.mod_ap.iter_mut()).chain(self.ap2.iter_mut()) {
            ap.line.clear();
        }
        for d in self.delay1.iter_mut().chain(self.delay2.iter_mut()) {
            d.clear();
        }
        self.damp = [0.0; 2];
        self.low_cut.reset();
        self.high_cut.reset();
    }

    fn tail(&self) -> usize {
        // Time for the tank to fall 60 dB: each loop (~0.3 s at size 1) loses
        // 20·log10(decay) dB.
        let loop_len = (DELAY1[0] + DELAY2[0] + MOD_AP[0] + AP2[0]) * self.scale;
        let loss_db = -20.0 * self.params.decay.clamp(0.01, 0.98).log10();
        (loop_len * (60.0 / loss_db)).ceil() as usize + self.rate as usize / 5
    }
}

impl Reverb {
    #[inline]
    fn tap(&self, branch: usize, which: usize, delay: f32) -> f32 {
        match which {
            0 => self.delay1[branch].read_frac(delay),
            1 => self.ap2[branch].line.read_frac(delay),
            _ => self.delay2[branch].read_frac(delay),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_response_decays_and_is_stereo() {
        let rate = 48_000;
        let mut rv = Reverb::new(rate, ReverbParams { mix: 1.0, decay: 0.6, ..Default::default() });
        let n = rate as usize * 3;
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        l[0] = 1.0;
        r[0] = 1.0;
        for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            rv.process(a, b);
        }
        let energy = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
        let e_early = energy(&l[0..rate as usize / 2]);
        let e_mid = energy(&l[rate as usize..rate as usize * 3 / 2]);
        let e_late = energy(&l[rate as usize * 5 / 2..]);
        assert!(e_early > e_mid && e_mid > e_late, "{e_early} {e_mid} {e_late}");
        assert!(e_late < e_early * 0.05);
        assert!(!l.iter().any(|v| v.is_nan()));
        // Left and right differ (decorrelated taps).
        let diff: f32 = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0, "{diff}");
        // Pre-delay: nothing before 10 ms.
        assert!(l[..400].iter().all(|v| v.abs() < 1e-6));
        assert!(rv.tail() > rate as usize);
    }

    #[test]
    fn dry_mix_is_identity_and_sizes_fit_their_buffers() {
        let rate = 96_000;
        for size in [0.3, 1.0, 2.0] {
            let mut rv = Reverb::new(rate, ReverbParams { size, mix: 0.0, ..Default::default() });
            let mut l: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.01).sin()).collect();
            let l0 = l.clone();
            let mut r = l.clone();
            rv.process(&mut l, &mut r);
            assert_eq!(l, l0);
            rv.set_params(ReverbParams { size, mix: 0.5, ..Default::default() });
            rv.process(&mut l, &mut r);
            assert!(l.iter().all(|v| v.is_finite()));
        }
    }
}
