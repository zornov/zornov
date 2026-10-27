// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dynamics processors: compressor, true-peak limiter, noise gate and
//! de-esser, plus the envelope follower and true-peak estimator they share.
//!
//! The compressor follows the feed-forward log-domain design analysed in
//! Giannoulis, Massberg & Reiss, "Digital Dynamic Range Compressor Design —
//! A Tutorial and Analysis" (JAES 2012): level detection, a soft-knee static
//! curve in dB, and a branching one-pole smoother on the gain reduction.
//! The limiter adds a look-ahead delay with a sliding minimum so no sample
//! ever passes the ceiling, and measures peaks after 4× oversampling as
//! ITU-R BS.1770-4 Annex 2 describes.

use crate::buffer::{Effect, db_to_gain, gain_to_db};
use crate::eq::{BandKind, Biquad, Coeffs};

/// One-pole time constant coefficient for `ms` milliseconds at `rate`.
#[inline]
pub fn time_coeff(ms: f32, rate: u32) -> f32 {
    if ms <= 0.0 { 0.0 } else { (-1.0 / (ms * 0.001 * rate as f32)).exp() }
}

/// Peak or RMS level detector with separate attack and release.
#[derive(Clone, Copy, Debug)]
pub struct EnvelopeFollower {
    attack: f32,
    release: f32,
    rms: bool,
    state: f32,
}

impl EnvelopeFollower {
    /// A follower at `rate`. `rms` averages power (then takes the root) instead
    /// of tracking the rectified signal.
    pub fn new(rate: u32, attack_ms: f32, release_ms: f32, rms: bool) -> Self {
        Self { attack: time_coeff(attack_ms, rate), release: time_coeff(release_ms, rate), rms, state: 0.0 }
    }

    /// Feeds one (already rectified or squared) sample and returns the level.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let v = if self.rms { x * x } else { x.abs() };
        let c = if v > self.state { self.attack } else { self.release };
        self.state = v + c * (self.state - v);
        if self.rms { self.state.sqrt() } else { self.state }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// Soft-knee static gain computer: returns the gain *reduction* in dB (≥ 0)
/// for an input level `x_db`.
#[inline]
pub fn knee_reduction_db(x_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let over = x_db - threshold_db;
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if knee_db <= 0.0 || over <= -knee_db / 2.0 {
        if over <= 0.0 { 0.0 } else { over * slope }
    } else if over >= knee_db / 2.0 {
        over * slope
    } else {
        let t = over + knee_db / 2.0;
        slope * t * t / (2.0 * knee_db)
    }
}

/// How the compressor measures level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Detector {
    /// Fast, follows transients (drums, speech plosives).
    #[default]
    Peak,
    /// Averaged power, closer to loudness (music beds, voice levelling).
    Rms,
}

/// Compressor settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompressorParams {
    /// Level above which gain reduction starts (−60..0 dB).
    pub threshold_db: f32,
    /// Compression ratio (1..20; 20 behaves as a limiter).
    pub ratio: f32,
    /// Width of the soft knee in dB (0 = hard).
    pub knee_db: f32,
    /// Attack time in ms (0.1..200).
    pub attack_ms: f32,
    /// Release time in ms (10..2000).
    pub release_ms: f32,
    /// Fixed make-up gain in dB.
    pub makeup_db: f32,
    /// Add make-up gain so a signal at the threshold stays level.
    pub auto_makeup: bool,
    /// Level detector.
    pub detector: Detector,
    /// High-pass on the detector only (keeps bass from pumping the mix).
    pub sidechain_hp_hz: Option<f32>,
    /// Dry/wet mix for parallel compression (0..1).
    pub mix: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self { threshold_db: -18.0, ratio: 3.0, knee_db: 6.0, attack_ms: 10.0, release_ms: 120.0, makeup_db: 0.0, auto_makeup: false, detector: Detector::Peak, sidechain_hp_hz: None, mix: 1.0 }
    }
}

impl CompressorParams {
    /// A gentle voice levelling preset.
    pub const VOICE: Self = Self { threshold_db: -20.0, ratio: 2.5, knee_db: 8.0, attack_ms: 8.0, release_ms: 150.0, makeup_db: 0.0, auto_makeup: true, detector: Detector::Rms, sidechain_hp_hz: Some(120.0), mix: 1.0 };
}

/// Stereo-linked feed-forward compressor.
pub struct Compressor {
    rate: u32,
    params: CompressorParams,
    follower: EnvelopeFollower,
    gr_db: f32,
    attack: f32,
    release: f32,
    sidechain: Option<Biquad>,
    last_gr: f32,
}

impl Compressor {
    /// Builds a compressor at `rate`.
    pub fn new(rate: u32, params: CompressorParams) -> Self {
        let mut c = Self { rate, params, follower: EnvelopeFollower::new(rate, 0.0, 0.0, false), gr_db: 0.0, attack: 0.0, release: 0.0, sidechain: None, last_gr: 0.0 };
        c.set_params(params);
        c
    }

    /// Current settings.
    pub fn params(&self) -> &CompressorParams {
        &self.params
    }

    /// Applies new settings (allocation-free).
    pub fn set_params(&mut self, p: CompressorParams) {
        self.params = p;
        // The detector rises almost instantly and holds peaks for ~30 ms so
        // a steady tone reads as its peak, not its rectified waveform; RMS
        // mode averages over 5 ms. The attack/release smoothing is on the gain.
        let rms = p.detector == Detector::Rms;
        let (det_a, det_r) = if rms { (5.0, 5.0) } else { (0.02, 30.0) };
        let keep = self.follower.state;
        self.follower = EnvelopeFollower::new(self.rate, det_a, det_r, rms);
        self.follower.state = keep;
        self.attack = time_coeff(p.attack_ms.max(0.01), self.rate);
        self.release = time_coeff(p.release_ms.max(1.0), self.rate);
        match p.sidechain_hp_hz {
            Some(f) => {
                let c = Coeffs::design(BandKind::HighPass, self.rate, f, 0.707, 0.0);
                match &mut self.sidechain {
                    Some(b) => b.coeffs = c,
                    None => self.sidechain = Some(Biquad::new(c)),
                }
            }
            None => self.sidechain = None,
        }
    }

    /// Gain reduction in dB (≥ 0) at the end of the last block, for meters.
    pub fn gain_reduction_db(&self) -> f32 {
        self.last_gr
    }

    fn makeup_db(&self) -> f32 {
        let p = &self.params;
        let auto = if p.auto_makeup { knee_reduction_db(0.0, p.threshold_db, p.ratio, p.knee_db) * 0.5 } else { 0.0 };
        p.makeup_db + auto
    }
}

impl Effect for Compressor {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let p = self.params;
        let makeup = db_to_gain(self.makeup_db());
        let mix = p.mix.clamp(0.0, 1.0);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mut key = l.abs().max(r.abs());
            if let Some(sc) = &mut self.sidechain {
                // Detect on the mono sum through the high-pass; the key stays
                // positive through abs().
                key = sc.tick(0, 0.5 * (*l + *r)).abs();
            }
            let level = self.follower.tick(key);
            let x_db = gain_to_db(level.max(1e-9));
            let target = knee_reduction_db(x_db, p.threshold_db, p.ratio, p.knee_db);
            let c = if target > self.gr_db { self.attack } else { self.release };
            self.gr_db = target + c * (self.gr_db - target);
            let g = db_to_gain(-self.gr_db) * makeup;
            let wet_l = *l * g;
            let wet_r = *r * g;
            *l += (wet_l - *l) * mix;
            *r += (wet_r - *r) * mix;
        }
        self.last_gr = self.gr_db;
    }

    fn reset(&mut self) {
        self.follower.reset();
        self.gr_db = 0.0;
        self.last_gr = 0.0;
        if let Some(sc) = &mut self.sidechain {
            sc.reset();
        }
    }
}

/// Number of oversampling phases used for true-peak estimation.
pub const TP_OVERSAMPLE: usize = 4;
const TP_TAPS: usize = 12;

/// Interpolating FIR that estimates the inter-sample peak of a signal at 4×
/// the sample rate (BS.1770-4 Annex 2 style). Introduces `TP_TAPS / 2`
/// samples of delay in the peak reading, which the limiter's look-ahead covers.
#[derive(Clone, Debug)]
pub struct TruePeak {
    taps: [[f32; TP_TAPS]; TP_OVERSAMPLE],
    hist: [[f32; TP_TAPS]; 2],
    idx: usize,
}

impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeak {
    /// Builds the interpolation filter.
    pub fn new() -> Self {
        let mut taps = [[0f32; TP_TAPS]; TP_OVERSAMPLE];
        let half = TP_TAPS as f64 / 2.0;
        for (p, row) in taps.iter_mut().enumerate() {
            let frac = p as f64 / TP_OVERSAMPLE as f64;
            let mut sum = 0.0;
            for (k, c) in row.iter_mut().enumerate() {
                let x = k as f64 - half + 1.0 - frac;
                let s = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                // Blackman window.
                let w = (k as f64 + 1.0 - frac) / TP_TAPS as f64;
                let win = 0.42 - 0.5 * (std::f64::consts::TAU * w).cos() + 0.08 * (2.0 * std::f64::consts::TAU * w).cos();
                *c = (s * win) as f32;
                sum += s * win;
            }
            for c in row.iter_mut() {
                *c /= sum as f32;
            }
        }
        Self { taps, hist: [[0.0; TP_TAPS]; 2], idx: 0 }
    }

    /// Pushes one stereo frame and returns the largest absolute oversampled
    /// value seen around it (across both channels).
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32) -> f32 {
        self.hist[0][self.idx] = l;
        self.hist[1][self.idx] = r;
        self.idx = (self.idx + 1) % TP_TAPS;
        let mut peak = 0.0f32;
        for ch in 0..2 {
            for row in &self.taps {
                let mut acc = 0.0;
                for (k, c) in row.iter().enumerate() {
                    acc += c * self.hist[ch][(self.idx + k) % TP_TAPS];
                }
                peak = peak.max(acc.abs());
            }
        }
        peak
    }

    /// Largest oversampled peak over whole planar buffers (allocates nothing).
    pub fn scan(&mut self, left: &[f32], right: &[f32]) -> f32 {
        let mut peak = 0.0f32;
        for (l, r) in left.iter().zip(right) {
            peak = peak.max(self.tick(*l, *r));
        }
        // Drain the filter so the last samples are seen too.
        for _ in 0..TP_TAPS {
            peak = peak.max(self.tick(0.0, 0.0));
        }
        peak
    }

    /// Clears the history.
    pub fn reset(&mut self) {
        self.hist = [[0.0; TP_TAPS]; 2];
        self.idx = 0;
    }
}

/// Limiter settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterParams {
    /// Output ceiling in dBTP (−20..0; −1 dBTP is the usual delivery value).
    pub ceiling_db: f32,
    /// Release time in ms (10..1000).
    pub release_ms: f32,
    /// Measure peaks after 4× oversampling (true peak) rather than per sample.
    pub true_peak: bool,
}

impl Default for LimiterParams {
    fn default() -> Self {
        Self { ceiling_db: -1.0, release_ms: 80.0, true_peak: true }
    }
}

const LIMITER_LOOKAHEAD_MS: f32 = 5.0;

/// Look-ahead brick-wall limiter with true-peak detection. Adds
/// `latency()` frames of delay.
pub struct Limiter {
    params: LimiterParams,
    lookahead: usize,
    delay: [Vec<f32>; 2],
    write: usize,
    // Monotonic deque of (index, gain) for the sliding minimum.
    deque_idx: Vec<usize>,
    deque_val: Vec<f32>,
    dq_head: usize,
    dq_tail: usize,
    dq_len: usize,
    min_hist: Vec<f32>,
    min_sum: f64,
    n: usize,
    tp: TruePeak,
    release: f32,
    gr_smooth: f32,
    last_gr: f32,
}

impl Limiter {
    /// Builds a limiter at `rate`.
    pub fn new(rate: u32, params: LimiterParams) -> Self {
        let lookahead = ((LIMITER_LOOKAHEAD_MS * 0.001 * rate as f32) as usize).max(TP_TAPS);
        let cap = lookahead + 1;
        let mut l = Self {
            params,
            lookahead,
            delay: [vec![0.0; cap], vec![0.0; cap]],
            write: 0,
            deque_idx: vec![0; cap + 1],
            deque_val: vec![1.0; cap + 1],
            dq_head: 0,
            dq_tail: 0,
            dq_len: 0,
            min_hist: vec![1.0; lookahead],
            min_sum: lookahead as f64,
            n: 0,
            tp: TruePeak::new(),
            release: 0.0,
            gr_smooth: 0.0,
            last_gr: 0.0,
        };
        l.set_params(params, rate);
        l
    }

    /// Applies new settings.
    pub fn set_params(&mut self, p: LimiterParams, rate: u32) {
        self.params = p;
        self.release = time_coeff(p.release_ms.max(1.0), rate);
    }

    /// Current settings.
    pub fn params(&self) -> &LimiterParams {
        &self.params
    }

    /// Gain reduction in dB at the end of the last block.
    pub fn gain_reduction_db(&self) -> f32 {
        self.last_gr
    }

    fn deque_push(&mut self, idx: usize, val: f32) {
        // Drop larger values from the back: they can never be the minimum
        // while `val` is in the window.
        while self.dq_len > 0 {
            let back = (self.dq_tail + self.deque_idx.len() - 1) % self.deque_idx.len();
            if self.deque_val[back] >= val {
                self.dq_tail = back;
                self.dq_len -= 1;
            } else {
                break;
            }
        }
        self.deque_idx[self.dq_tail] = idx;
        self.deque_val[self.dq_tail] = val;
        self.dq_tail = (self.dq_tail + 1) % self.deque_idx.len();
        self.dq_len += 1;
    }

    fn deque_expire(&mut self, oldest_valid: usize) {
        while self.dq_len > 0 && self.deque_idx[self.dq_head] < oldest_valid {
            self.dq_head = (self.dq_head + 1) % self.deque_idx.len();
            self.dq_len -= 1;
        }
    }
}

impl Effect for Limiter {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let ceiling = db_to_gain(self.params.ceiling_db);
        let cap = self.delay[0].len();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let x_l = *l;
            let x_r = *r;
            // Required gain for this input frame.
            let peak = if self.params.true_peak { self.tp.tick(x_l, x_r) } else { x_l.abs().max(x_r.abs()) };
            let needed_db = if peak > ceiling { gain_to_db(peak / ceiling) } else { 0.0 };
            // Release smoothing on the reduction (instant attack: the
            // look-ahead window handles the attack shape).
            self.gr_smooth = if needed_db > self.gr_smooth { needed_db } else { needed_db + self.release * (self.gr_smooth - needed_db) };
            let g = db_to_gain(-self.gr_smooth);
            // Sliding minimum over the last lookahead + 1 gains.
            self.deque_push(self.n, g);
            self.deque_expire(self.n.saturating_sub(self.lookahead));
            let g_min = self.deque_val[self.dq_head];
            // Box smoothing of the minimum over `lookahead` samples.
            let slot = self.n % self.lookahead;
            self.min_sum += g_min as f64 - self.min_hist[slot] as f64;
            self.min_hist[slot] = g_min;
            let g_apply = (self.min_sum / self.lookahead as f64) as f32;
            // Delay line: write the input, read lookahead frames back.
            let read = (self.write + cap - self.lookahead) % cap;
            let d_l = self.delay[0][read];
            let d_r = self.delay[1][read];
            self.delay[0][self.write] = x_l;
            self.delay[1][self.write] = x_r;
            self.write = (self.write + 1) % cap;
            self.n += 1;
            *l = d_l * g_apply;
            *r = d_r * g_apply;
            self.last_gr = -gain_to_db(g_apply.max(1e-9));
        }
    }

    fn reset(&mut self) {
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.write = 0;
        self.dq_head = 0;
        self.dq_tail = 0;
        self.dq_len = 0;
        self.min_hist.fill(1.0);
        self.min_sum = self.lookahead as f64;
        self.n = 0;
        self.tp.reset();
        self.gr_smooth = 0.0;
        self.last_gr = 0.0;
    }

    fn latency(&self) -> usize {
        self.lookahead
    }
}

/// Noise gate settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateParams {
    /// Level above which the gate opens (−80..0 dB).
    pub threshold_db: f32,
    /// How far below the open threshold the level must fall to close (0..20 dB).
    pub hysteresis_db: f32,
    /// Attenuation when closed (0..100 dB; 100 is effectively off).
    pub range_db: f32,
    /// Opening time in ms.
    pub attack_ms: f32,
    /// Minimum open time after the level drops, in ms.
    pub hold_ms: f32,
    /// Closing time in ms.
    pub release_ms: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self { threshold_db: -40.0, hysteresis_db: 6.0, range_db: 80.0, attack_ms: 1.0, hold_ms: 50.0, release_ms: 100.0 }
    }
}

/// Noise gate (downward expander with hysteresis and hold).
pub struct Gate {
    params: GateParams,
    follower: EnvelopeFollower,
    open: bool,
    hold_left: usize,
    hold_frames: usize,
    gain: f32,
    attack: f32,
    release: f32,
    floor: f32,
}

impl Gate {
    /// Builds a gate at `rate`.
    pub fn new(rate: u32, params: GateParams) -> Self {
        let mut g = Self { params, follower: EnvelopeFollower::new(rate, 0.2, 5.0, true), open: false, hold_left: 0, hold_frames: 0, gain: 0.0, attack: 0.0, release: 0.0, floor: 0.0 };
        g.set_params(params, rate);
        g
    }

    /// Applies new settings.
    pub fn set_params(&mut self, p: GateParams, rate: u32) {
        self.params = p;
        self.hold_frames = (p.hold_ms.max(0.0) * 0.001 * rate as f32) as usize;
        self.attack = time_coeff(p.attack_ms.max(0.05), rate);
        self.release = time_coeff(p.release_ms.max(1.0), rate);
        self.floor = db_to_gain(-p.range_db.clamp(0.0, 100.0));
    }

    /// True while the gate is open (for a status light).
    pub fn is_open(&self) -> bool {
        self.open
    }
}

impl Effect for Gate {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let open_at = db_to_gain(self.params.threshold_db);
        let close_at = db_to_gain(self.params.threshold_db - self.params.hysteresis_db.max(0.0));
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let level = self.follower.tick(l.abs().max(r.abs()));
            if level > open_at {
                self.open = true;
                self.hold_left = self.hold_frames;
            } else if level < close_at {
                if self.hold_left > 0 {
                    self.hold_left -= 1;
                } else {
                    self.open = false;
                }
            }
            let target = if self.open { 1.0 } else { self.floor };
            let c = if target > self.gain { self.attack } else { self.release };
            self.gain = target + c * (self.gain - target);
            *l *= self.gain;
            *r *= self.gain;
        }
    }

    fn reset(&mut self) {
        self.follower.reset();
        self.open = false;
        self.hold_left = 0;
        self.gain = 0.0;
    }
}

/// De-esser settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeEsserParams {
    /// Centre of the sibilance band in Hz (3000..12000; 6500 suits most voices).
    pub freq_hz: f32,
    /// Band level above which reduction starts (−60..0 dB).
    pub threshold_db: f32,
    /// Maximum reduction in dB (0..24).
    pub range_db: f32,
    /// Reduce only the band above `freq_hz` (split) rather than the whole signal.
    pub split_band: bool,
}

impl Default for DeEsserParams {
    fn default() -> Self {
        Self { freq_hz: 6500.0, threshold_db: -30.0, range_db: 12.0, split_band: true }
    }
}

/// De-esser: a fast compressor keyed on the sibilance band, acting on the
/// band alone (split mode, through a 4th-order Linkwitz-Riley crossover) or on
/// the whole signal.
pub struct DeEsser {
    rate: u32,
    params: DeEsserParams,
    key: Biquad,
    follower: EnvelopeFollower,
    lp: [Biquad; 2],
    hp: [Biquad; 2],
    gr_db: f32,
    attack: f32,
    release: f32,
    last_gr: f32,
}

impl DeEsser {
    /// Builds a de-esser at `rate`.
    pub fn new(rate: u32, params: DeEsserParams) -> Self {
        let mut d = Self {
            rate,
            params,
            key: Biquad::default(),
            follower: EnvelopeFollower::new(rate, 0.5, 20.0, true),
            lp: [Biquad::default(); 2],
            hp: [Biquad::default(); 2],
            gr_db: 0.0,
            attack: time_coeff(0.5, rate),
            release: time_coeff(40.0, rate),
            last_gr: 0.0,
        };
        d.set_params(params);
        d
    }

    /// Applies new settings.
    pub fn set_params(&mut self, p: DeEsserParams) {
        self.params = p;
        let f = p.freq_hz.clamp(1000.0, 16_000.0);
        self.key.coeffs = Coeffs::design(BandKind::BandPass, self.rate, f, 1.2, 0.0);
        // Two cascaded Butterworth sections make a Linkwitz-Riley crossover
        // whose low and high outputs sum to an all-pass.
        let xo = f * 0.75;
        let lp = Coeffs::design(BandKind::LowPass, self.rate, xo, 0.707, 0.0);
        let hp = Coeffs::design(BandKind::HighPass, self.rate, xo, 0.707, 0.0);
        for b in &mut self.lp {
            b.coeffs = lp;
        }
        for b in &mut self.hp {
            b.coeffs = hp;
        }
    }

    /// Gain reduction in dB applied to the band at the end of the last block.
    pub fn gain_reduction_db(&self) -> f32 {
        self.last_gr
    }
}

impl Effect for DeEsser {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let p = self.params;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let mono = 0.5 * (*l + *r);
            let band = self.key.tick(0, mono);
            let level = self.follower.tick(band);
            let x_db = gain_to_db(level.max(1e-9));
            let target = knee_reduction_db(x_db, p.threshold_db, 6.0, 4.0).min(p.range_db.max(0.0));
            let c = if target > self.gr_db { self.attack } else { self.release };
            self.gr_db = target + c * (self.gr_db - target);
            let g = db_to_gain(-self.gr_db);
            if p.split_band {
                let (lp0, lp1) = self.lp.split_at_mut(1);
                let (hp0, hp1) = self.hp.split_at_mut(1);
                let lo_l = lp1[0].tick(0, lp0[0].tick(0, *l));
                let lo_r = lp1[0].tick(1, lp0[0].tick(1, *r));
                let hi_l = hp1[0].tick(0, hp0[0].tick(0, *l));
                let hi_r = hp1[0].tick(1, hp0[0].tick(1, *r));
                *l = lo_l + hi_l * g;
                *r = lo_r + hi_r * g;
            } else {
                *l *= g;
                *r *= g;
            }
        }
        self.last_gr = self.gr_db;
    }

    fn reset(&mut self) {
        self.key.reset();
        self.follower.reset();
        for b in self.lp.iter_mut().chain(self.hp.iter_mut()) {
            b.reset();
        }
        self.gr_db = 0.0;
        self.last_gr = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin()).collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn knee_curve_shape() {
        assert_eq!(knee_reduction_db(-30.0, -20.0, 4.0, 0.0), 0.0);
        assert!((knee_reduction_db(-10.0, -20.0, 4.0, 0.0) - 7.5).abs() < 1e-5);
        // Soft knee is continuous and between the hard-knee bounds.
        let soft = knee_reduction_db(-20.0, -20.0, 4.0, 6.0);
        assert!(soft > 0.0 && soft < 2.25);
        assert!((knee_reduction_db(-17.0, -20.0, 4.0, 6.0) - 2.25).abs() < 1e-5);
    }

    #[test]
    fn compressor_reduces_loud_steady_tone_by_ratio() {
        let rate = 48_000;
        let p = CompressorParams { threshold_db: -20.0, ratio: 4.0, knee_db: 0.0, attack_ms: 1.0, release_ms: 50.0, ..Default::default() };
        let mut c = Compressor::new(rate, p);
        // 0 dBFS peak tone: 20 dB over threshold → 15 dB reduction → −15 dBFS.
        let mut l = sine(1000.0, rate, rate as usize, 1.0);
        let mut r = l.clone();
        c.process(&mut l, &mut r);
        let out_db = gain_to_db(peak(&l[rate as usize / 2..]));
        assert!((out_db + 15.0).abs() < 0.4, "{out_db}");
        assert!((c.gain_reduction_db() - 15.0).abs() < 0.4);
        // Below threshold: untouched.
        c.reset();
        let mut l = sine(1000.0, rate, rate as usize, 0.05);
        let mut r = l.clone();
        c.process(&mut l, &mut r);
        assert!((peak(&l[1000..]) - 0.05).abs() < 1e-3);
    }

    #[test]
    fn true_peak_sees_between_samples() {
        // A full-scale sine at fs/4 sampled at phase 45° has sample peaks of
        // 0.707 but a true peak of 1.0.
        let rate = 48_000;
        let x: Vec<f32> = (0..4800).map(|i| (std::f32::consts::TAU * 12_000.0 * i as f32 / rate as f32 + std::f32::consts::FRAC_PI_4).sin()).collect();
        assert!((peak(&x) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        let mut tp = TruePeak::new();
        let tp_peak = tp.scan(&x, &x);
        assert!((tp_peak - 1.0).abs() < 0.03, "{tp_peak}");
    }

    #[test]
    fn limiter_never_passes_the_ceiling() {
        let rate = 48_000;
        let mut lim = Limiter::new(rate, LimiterParams { ceiling_db: -1.0, release_ms: 50.0, true_peak: true });
        // Bursts of +6 dB tone with silence in between.
        let n = rate as usize;
        let mut l = vec![0.0; n];
        for (i, v) in l.iter_mut().enumerate() {
            if (i / 4800) % 2 == 0 {
                *v = 2.0 * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin();
            }
        }
        let mut r = l.clone();
        let mut out_l = Vec::new();
        for ch in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            lim.process(ch.0, ch.1);
            out_l.extend_from_slice(ch.0);
        }
        let ceiling = db_to_gain(-1.0);
        assert!(peak(&out_l) <= ceiling * 1.001, "{} > {}", peak(&out_l), ceiling);
        // It actually got close to the ceiling (not just crushed).
        assert!(peak(&out_l) > ceiling * 0.9);
        // Latency is the look-ahead.
        assert_eq!(lim.latency(), 240);
    }

    #[test]
    fn limiter_is_transparent_below_ceiling() {
        let rate = 48_000;
        let mut lim = Limiter::new(rate, LimiterParams::default());
        let l0 = sine(440.0, rate, 4800, 0.5);
        let mut l = l0.clone();
        let mut r = l0.clone();
        lim.process(&mut l, &mut r);
        let d = lim.latency();
        for i in d..4800 {
            assert!((l[i] - l0[i - d]).abs() < 1e-6);
        }
    }

    #[test]
    fn gate_closes_on_noise_and_opens_on_signal() {
        let rate = 48_000;
        let mut g = Gate::new(rate, GateParams { threshold_db: -30.0, hysteresis_db: 6.0, range_db: 60.0, attack_ms: 1.0, hold_ms: 20.0, release_ms: 20.0 });
        let n = rate as usize / 2;
        let mut l: Vec<f32> = sine(500.0, rate, n, 0.005); // −46 dB: noise floor
        l.extend(sine(500.0, rate, n, 0.5));
        let mut r = l.clone();
        g.process(&mut l, &mut r);
        assert!(peak(&l[n / 2..n - 100]) < 0.005 * 0.01, "closed: {}", peak(&l[n / 2..n - 100]));
        assert!(peak(&l[n + 2400..]) > 0.49, "open");
        assert!(g.is_open());
    }

    #[test]
    fn de_esser_tames_sibilance_band_only() {
        let rate = 48_000;
        let mut d = DeEsser::new(rate, DeEsserParams { freq_hz: 7000.0, threshold_db: -40.0, range_db: 12.0, split_band: true });
        let n = rate as usize / 2;
        let low = sine(300.0, rate, n, 0.3);
        let ess = sine(7000.0, rate, n, 0.3);
        let mut l: Vec<f32> = low.iter().zip(&ess).map(|(a, b)| a + b).collect();
        let mut r = l.clone();
        d.process(&mut l, &mut r);
        assert!(d.gain_reduction_db() > 8.0, "{}", d.gain_reduction_db());
        // Project the output onto both tones over the settled half.
        let dot = |x: &[f32], y: &[f32]| x[n / 2..].iter().zip(&y[n / 2..]).map(|(a, b)| a * b).sum::<f32>() / (n / 2) as f32 * 2.0;
        let low_amp = dot(&l, &low) / 0.3;
        let ess_amp = dot(&l, &ess) / 0.3;
        assert!((low_amp - 0.3).abs() < 0.02, "low band kept: {low_amp}");
        assert!(ess_amp < 0.3 * 0.4, "sibilance reduced: {ess_amp}");
    }
}
