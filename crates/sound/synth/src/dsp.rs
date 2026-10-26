// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The small parts every sound is built from: a seeded random source,
//! a stable state-variable filter, band-limited oscillators, envelopes and
//! a Karplus-Strong string.

use std::f32::consts::{PI, TAU};

/// Sample rate of everything made here.
pub const RATE: u32 = 48_000;
/// [`RATE`] as a float.
pub const SR: f32 = RATE as f32;

/// Frequency of a (possibly fractional) MIDI note.
pub fn midi_hz(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

/// MIDI note of a frequency.
pub fn hz_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz.max(1e-3) / 440.0).log2()
}

/// Ratio of a pitch change in cents.
pub fn cents(c: f32) -> f32 {
    2f32.powf(c / 1200.0)
}

/// Gain of a level in dB.
pub fn db(d: f32) -> f32 {
    10f32.powf(d / 20.0)
}

/// Seeded random numbers (xorshift64*), the same sequence on every machine.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A source for `seed` (any value, zero included).
    pub fn new(seed: u64) -> Self {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self(z | 1)
    }
    /// The next 64 random bits.
    pub fn u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in 0..1.
    pub fn unit(&mut self) -> f32 {
        (self.u64() >> 40) as f32 / 16_777_216.0
    }
    /// Uniform in -1..1 (white noise).
    pub fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
    /// Uniform in `a..b`.
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
    /// True with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
    /// Uniform in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.u64() % n as u64) as usize }
    }
    /// Roughly normal with the given spread (sum of three uniforms).
    pub fn gauss(&mut self, spread: f32) -> f32 {
        (self.signed() + self.signed() + self.signed()) / 1.732 * spread
    }
    /// One of `items`.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    /// A source seeded from this one and a salt.
    pub fn fork(&mut self, salt: u64) -> Rng {
        Rng::new(self.u64() ^ salt.wrapping_mul(0x9E37_79B9))
    }
}

/// Filter coefficients for [`Svf`].
#[derive(Clone, Copy, Debug)]
pub struct SvfCoef {
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

impl SvfCoef {
    /// A filter at `hz` with quality `q` (0.5 is gentle, 10 rings).
    pub fn new(hz: f32, q: f32) -> Self {
        let hz = hz.clamp(10.0, SR * 0.45);
        let g = (PI * hz / SR).tan();
        let k = 1.0 / q.max(0.05);
        let a1 = 1.0 / (1.0 + g * (g + k));
        Self { a1, a2: g * a1, a3: g * g * a1, k }
    }
}

/// What a state-variable filter outputs at once.
#[derive(Clone, Copy, Debug, Default)]
pub struct SvfOut {
    /// Low-pass.
    pub lp: f32,
    /// Band-pass with unit gain at the centre.
    pub bp: f32,
    /// High-pass.
    pub hp: f32,
}

/// A state-variable filter that stays stable while its centre moves.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// A cleared filter.
    pub fn new() -> Self {
        Self::default()
    }
    /// Filters one sample.
    pub fn tick(&mut self, c: &SvfCoef, x: f32) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut { lp: v2, bp: c.k * v1, hp: x - c.k * v1 - v2 }
    }
    /// Filters one sample at a moving `hz`.
    pub fn at(&mut self, x: f32, hz: f32, q: f32) -> SvfOut {
        self.tick(&SvfCoef::new(hz, q), x)
    }
}

/// A one-pole low-pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lp1 {
    y: f32,
}

impl Lp1 {
    /// Smooths `x` with a corner at `hz`.
    pub fn tick(&mut self, x: f32, hz: f32) -> f32 {
        let a = 1.0 - (-TAU * hz.clamp(1.0, SR * 0.45) / SR).exp();
        self.y += a * (x - self.y);
        self.y
    }
}

/// The step ahead of a band-limited edge (PolyBLEP).
fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// A band-limited sawtooth.
#[derive(Clone, Copy, Debug, Default)]
pub struct Saw {
    ph: f32,
}

impl Saw {
    /// Starting at phase `ph` (0..1).
    pub fn at(ph: f32) -> Self {
        Self { ph: ph.rem_euclid(1.0) }
    }
    /// The next sample at `hz`.
    pub fn tick(&mut self, hz: f32) -> f32 {
        let dt = (hz / SR).clamp(1e-5, 0.45);
        let v = 2.0 * self.ph - 1.0 - blep(self.ph, dt);
        self.ph += dt;
        if self.ph >= 1.0 {
            self.ph -= 1.0;
        }
        v
    }
}

/// A band-limited pulse wave.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pulse {
    ph: f32,
}

impl Pulse {
    /// Starting at phase `ph` (0..1).
    pub fn at(ph: f32) -> Self {
        Self { ph: ph.rem_euclid(1.0) }
    }
    /// The next sample at `hz` with duty `w` (0.5 is a square).
    pub fn tick(&mut self, hz: f32, w: f32) -> f32 {
        let dt = (hz / SR).clamp(1e-5, 0.45);
        let w = w.clamp(0.05, 0.95);
        let mut v = if self.ph < w { 1.0 } else { -1.0 };
        v += blep(self.ph, dt);
        v -= blep((self.ph - w + 1.0).rem_euclid(1.0), dt);
        self.ph += dt;
        if self.ph >= 1.0 {
            self.ph -= 1.0;
        }
        v
    }
}

/// A sine oscillator made by rotating a point: no `sin` call per sample.
#[derive(Clone, Copy, Debug)]
pub struct Rot {
    c: f32,
    s: f32,
    dc: f32,
    ds: f32,
}

impl Rot {
    /// A sine at `hz`, starting at `phase` radians.
    pub fn new(hz: f32, phase: f32) -> Self {
        let w = TAU * hz / SR;
        Self { c: phase.cos(), s: phase.sin(), dc: w.cos(), ds: w.sin() }
    }
    /// The next sample.
    pub fn tick(&mut self) -> f32 {
        let o = self.s;
        let c = self.c * self.dc - self.s * self.ds;
        self.s = self.s * self.dc + self.c * self.ds;
        self.c = c;
        o
    }
}

/// Smooth 0..1 ramp (cubic) over `len` seconds.
pub fn smooth(t: f32, len: f32) -> f32 {
    let x = (t / len.max(1e-5)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A held note's level: a smooth attack, a hold, then an exponential release
/// that dies away with time constant `rel`.
pub fn asr(t: f32, hold: f32, attack: f32, rel: f32) -> f32 {
    if t < hold {
        smooth(t, attack)
    } else {
        smooth(hold, attack) * (-(t - hold) / rel.max(1e-4)).exp()
    }
}

/// How many seconds a release with constant `rel` needs to fall below -60 dB.
pub fn release_len(rel: f32) -> f32 {
    rel * 6.9
}

/// Number of samples for `seconds`.
pub fn frames(seconds: f32) -> usize {
    (seconds.max(0.0) * SR).round() as usize
}

/// Cents of vibrato at time `t`: it fades in after `delay` seconds.
pub fn vibrato(t: f32, rate: f32, depth_cents: f32, delay: f32) -> f32 {
    depth_cents * smooth(t - delay, 0.4) * (TAU * rate * t).sin()
}

/// A plucked or struck string: a delay loop with a damping filter, tuned so
/// the loop's total delay (including the filter's) matches the pitch.
pub struct Ks {
    buf: Vec<f32>,
    w: usize,
    n: usize,
    frac: f32,
    a: f32,
    gain: f32,
    y: f32,
}

impl Ks {
    /// A string at `hz`. `bright` (0..1) is how little the loop damps the
    /// highs; `t60` is how long it takes to fall 60 dB; `excite` fills the
    /// first period.
    pub fn new(hz: f32, bright: f32, t60: f32, excite: &[f32]) -> Self {
        let hz = hz.clamp(20.0, SR * 0.2);
        let a = (0.15 + 0.85 * bright).clamp(0.05, 1.0);
        let w0 = TAU * hz / SR;
        // Phase delay of the one-pole loop filter at the fundamental.
        let phase = ((1.0 - a) * w0.sin()).atan2(1.0 - (1.0 - a) * w0.cos());
        let d = (SR / hz - phase / w0).max(2.0);
        let n = d.floor() as usize;
        let frac = d - n as f32;
        let cap = n + 3;
        let mut buf = vec![0.0; cap];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = excite.get(i % excite.len().max(1)).copied().unwrap_or(0.0);
        }
        let gain = 10f32.powf(-3.0 * d / (SR * t60.max(0.05))).min(0.9999);
        Self { buf, w: 0, n, frac, a, gain, y: 0.0 }
    }
    /// The next sample.
    pub fn tick(&mut self) -> f32 {
        let cap = self.buf.len();
        let i0 = (self.w + cap - self.n) % cap;
        let i1 = (self.w + cap - self.n - 1) % cap;
        let x = (1.0 - self.frac) * self.buf[i0] + self.frac * self.buf[i1];
        self.y += self.a * (x - self.y);
        let out = self.y * self.gain;
        self.buf[self.w] = out;
        self.w = (self.w + 1) % cap;
        out
    }
}

/// An exciter for a plucked string: noise shaped by a low-pass (`bright`
/// 0..1), with the pluck position `pos` (fraction of the string) taking out
/// the harmonics that would have a node there.
pub fn pluck_excite(rng: &mut Rng, period: usize, bright: f32, pos: f32) -> Vec<f32> {
    let n = period.max(4);
    let mut lp = Lp1::default();
    let hz = 400.0 + 9000.0 * bright.clamp(0.0, 1.0);
    let mut e: Vec<f32> = (0..n).map(|_| lp.tick(rng.signed(), hz)).collect();
    let mean = e.iter().sum::<f32>() / n as f32;
    e.iter_mut().for_each(|v| *v -= mean);
    let k = ((pos.clamp(0.05, 0.5)) * n as f32) as usize;
    let copy = e.clone();
    for i in 0..n {
        e[i] -= copy[(i + n - k) % n];
    }
    let peak = e.iter().fold(1e-6f32, |m, v| m.max(v.abs()));
    e.iter_mut().for_each(|v| *v /= peak);
    e
}

/// Filters a whole buffer with a band-pass.
pub fn band_pass(x: &[f32], hz: f32, q: f32) -> Vec<f32> {
    let c = SvfCoef::new(hz, q);
    let mut f = Svf::new();
    x.iter().map(|v| f.tick(&c, *v).bp).collect()
}

/// Filters a whole buffer with a low-pass.
pub fn low_pass(x: &[f32], hz: f32, q: f32) -> Vec<f32> {
    let c = SvfCoef::new(hz, q);
    let mut f = Svf::new();
    x.iter().map(|v| f.tick(&c, *v).lp).collect()
}

/// Filters a whole buffer with a high-pass.
pub fn high_pass(x: &[f32], hz: f32, q: f32) -> Vec<f32> {
    let c = SvfCoef::new(hz, q);
    let mut f = Svf::new();
    x.iter().map(|v| f.tick(&c, *v).hp).collect()
}

/// Adds `src` into `dst` starting at sample `at` (clipped at the end).
pub fn mix_into(dst: &mut [f32], src: &[f32], at: usize, gain: f32) {
    if at >= dst.len() {
        return;
    }
    for (d, s) in dst[at..].iter_mut().zip(src) {
        *d += s * gain;
    }
}

/// Root-mean-square level.
pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// Largest absolute sample.
pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// A stereo signal at [`RATE`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stereo {
    /// Left channel.
    pub l: Vec<f32>,
    /// Right channel.
    pub r: Vec<f32>,
}

impl Stereo {
    /// Silence of `n` frames.
    pub fn silent(n: usize) -> Self {
        Self { l: vec![0.0; n], r: vec![0.0; n] }
    }
    /// Frames.
    pub fn len(&self) -> usize {
        self.l.len()
    }
    /// True when there are no frames.
    pub fn is_empty(&self) -> bool {
        self.l.is_empty()
    }
    /// Seconds.
    pub fn seconds(&self) -> f32 {
        self.l.len() as f32 / SR
    }
    /// Both channels of a mono signal.
    pub fn from_mono(m: &[f32]) -> Self {
        Self { l: m.to_vec(), r: m.to_vec() }
    }
    /// A mono signal placed with `pan` (-1 left .. 1 right), equal power.
    pub fn panned(m: &[f32], pan: f32) -> Self {
        let p = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * PI;
        let (gl, gr) = (p.cos(), p.sin());
        Self { l: m.iter().map(|v| v * gl).collect(), r: m.iter().map(|v| v * gr).collect() }
    }
    /// Adds `other` starting at sample `at`.
    pub fn add_at(&mut self, other: &Stereo, at: usize, gain: f32) {
        mix_into(&mut self.l, &other.l, at, gain);
        mix_into(&mut self.r, &other.r, at, gain);
    }
    /// Adds a mono signal with `pan` starting at sample `at`.
    pub fn add_mono(&mut self, m: &[f32], at: usize, gain: f32, pan: f32) {
        let p = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * PI;
        mix_into(&mut self.l, m, at, gain * p.cos());
        mix_into(&mut self.r, m, at, gain * p.sin());
    }
    /// Largest absolute sample on either channel.
    pub fn peak(&self) -> f32 {
        peak(&self.l).max(peak(&self.r))
    }
    /// Scales both channels.
    pub fn scale(&mut self, g: f32) {
        self.l.iter_mut().chain(self.r.iter_mut()).for_each(|v| *v *= g);
    }
    /// Cuts or pads with silence to exactly `n` frames.
    pub fn fit(&mut self, n: usize) {
        self.l.resize(n, 0.0);
        self.r.resize(n, 0.0);
    }
    /// Fades the last `seconds` to silence and the first few milliseconds
    /// in, so nothing clicks at either edge.
    pub fn edges(&mut self, fade_out: f32) {
        let n = self.len();
        let fi = frames(0.004).min(n / 2);
        let fo = frames(fade_out).min(n / 2);
        for i in 0..n {
            let mut g = 1.0;
            if i < fi {
                g = i as f32 / fi as f32;
            }
            if fo > 0 && i + fo > n {
                g *= (n - i) as f32 / fo as f32;
            }
            self.l[i] *= g;
            self.r[i] *= g;
        }
    }
    /// 16-bit stereo WAV file bytes.
    pub fn wav_bytes(&self) -> Vec<u8> {
        wav_bytes(&self.l, &self.r)
    }
    /// Interleaved samples.
    pub fn interleaved(&self) -> Vec<f32> {
        self.l.iter().zip(&self.r).flat_map(|(a, b)| [*a, *b]).collect()
    }
}

/// 16-bit stereo WAV file bytes at [`RATE`].
pub fn wav_bytes(l: &[f32], r: &[f32]) -> Vec<u8> {
    let n = l.len().min(r.len());
    let data = (n * 4) as u32;
    let mut out = Vec::with_capacity(44 + n * 4);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..n {
        for v in [l[i], r[i]] {
            out.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_repeatable_and_spread() {
        let (mut a, mut b) = (Rng::new(5), Rng::new(5));
        assert_eq!(a.u64(), b.u64());
        let mean: f32 = (0..2000).map(|_| a.unit()).sum::<f32>() / 2000.0;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
    }

    #[test]
    fn string_is_in_tune() {
        for hz in [55.0f32, 110.0, 220.0, 440.0, 880.0] {
            let mut rng = Rng::new(3);
            let ex = pluck_excite(&mut rng, (SR / hz) as usize, 0.6, 0.2);
            let mut s = Ks::new(hz, 0.6, 3.0, &ex);
            let x: Vec<f32> = (0..frames(1.0)).map(|_| s.tick()).collect();
            let got = studio_sound::pitch::dominant_frequency(&x[2000..], RATE, hz * 0.7, hz * 1.4).unwrap();
            assert!((got / hz - 1.0).abs() < 0.01, "{hz}: {got}");
        }
    }

    #[test]
    fn filters_pass_what_they_should() {
        let mut rng = Rng::new(1);
        let n: Vec<f32> = (0..frames(1.0)).map(|_| rng.signed()).collect();
        let lo = low_pass(&n, 300.0, 0.7);
        let hi = high_pass(&n, 6000.0, 0.7);
        assert!(rms(&lo) < rms(&n) * 0.5 && rms(&hi) < rms(&n) * 0.93);
        assert!(lo.iter().chain(&hi).all(|v| v.is_finite()));
    }
}
