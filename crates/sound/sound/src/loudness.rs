// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Loudness metering and normalisation after ITU-R BS.1770-4 (the basis of
//! EBU R 128): K-weighting (a high shelf then a high-pass), mean square over
//! 400 ms blocks overlapping by 75 %, absolute gating at −70 LUFS and
//! relative gating 10 LU under the ungated mean, plus short-term (3 s) and
//! momentary (400 ms) values, loudness range (EBU Tech 3342) and true peak.
//!
//! Gated loudness and range are computed from fixed histograms rather than
//! by keeping every block, so the meter never allocates while running and
//! can sit on the monitoring path.

use crate::buffer::gain_to_db;
use crate::dynamics::TruePeak;
use crate::eq::{Biquad, Coeffs};
use crate::multi::MultiBuf;
use crate::surround::Layout;

/// Offset that makes a full-scale 1 kHz sine read 0 LUFS... minus 0.691, as
/// the standard defines loudness: `L = −0.691 + 10·log10(Σ G·z)`.
const LUFS_OFFSET: f64 = -0.691;
const ABSOLUTE_GATE: f64 = -70.0;
const RELATIVE_GATE: f64 = -10.0;
const HIST_MIN: f64 = -70.0;
const HIST_MAX: f64 = 10.0;
const HIST_STEP: f64 = 0.1;
const HIST_BINS: usize = ((HIST_MAX - HIST_MIN) / HIST_STEP) as usize + 1;

/// The K-weighting pre-filter pair, designed for any sample rate from the
/// standard's analogue prototype (the published 48 kHz coefficients fall out
/// exactly; see the tests).
#[derive(Clone, Debug)]
pub struct KWeighting {
    shelf: Biquad,
    high_pass: Biquad,
}

impl KWeighting {
    /// Filters for `rate`.
    pub fn new(rate: u32) -> Self {
        Self { shelf: Biquad::new(Self::shelf_coeffs(rate)), high_pass: Biquad::new(Self::high_pass_coeffs(rate)) }
    }

    /// Stage 1: a +4 dB high shelf around 1.68 kHz modelling the head's
    /// acoustic effect. Bilinear transform of the second-order shelf
    /// prototype `(Vh·s² + Vb·(ω/Q)·s + ω²) / (s² + (ω/Q)·s + ω²)`.
    pub fn shelf_coeffs(rate: u32) -> Coeffs {
        let f0 = 1681.974_450_955_533;
        let gain_db = 3.999_843_853_973_347;
        let q = 0.707_175_236_955_419_6;
        let k = (std::f64::consts::PI * f0 / rate as f64).tan();
        let vh = 10f64.powf(gain_db / 20.0);
        // The standard's shelf is very slightly asymmetric; this exponent
        // (rather than 0.5) reproduces its tabulated coefficients.
        let vb = vh.powf(0.499_666_774_154_541_6);
        let a0 = 1.0 + k / q + k * k;
        Coeffs {
            b0: ((vh + vb * k / q + k * k) / a0) as f32,
            b1: (2.0 * (k * k - vh) / a0) as f32,
            b2: ((vh - vb * k / q + k * k) / a0) as f32,
            a1: (2.0 * (k * k - 1.0) / a0) as f32,
            a2: ((1.0 - k / q + k * k) / a0) as f32,
        }
    }

    /// Stage 2: the RLB weighting, a second-order high-pass at 38 Hz. As in
    /// the standard, the numerator is left at `1, −2, 1` (a hair above unity
    /// gain at high frequencies) rather than normalised.
    pub fn high_pass_coeffs(rate: u32) -> Coeffs {
        let f0 = 38.135_470_876_024_44;
        let q = 0.500_327_037_323_877_3;
        let k = (std::f64::consts::PI * f0 / rate as f64).tan();
        let a0 = 1.0 + k / q + k * k;
        Coeffs { b0: 1.0, b1: -2.0, b2: 1.0, a1: (2.0 * (k * k - 1.0) / a0) as f32, a2: ((1.0 - k / q + k * k) / a0) as f32 }
    }

    /// Filters one sample of channel `ch` (0 or 1).
    #[inline]
    pub fn tick(&mut self, ch: usize, x: f32) -> f32 {
        self.high_pass.tick(ch, self.shelf.tick(ch, x))
    }

    /// Clears the filter state.
    pub fn reset(&mut self) {
        self.shelf.reset();
        self.high_pass.reset();
    }
}

/// Fixed 0.1 LU histogram over −70..+10 LUFS of block loudness values.
#[derive(Clone, Debug)]
struct Histogram {
    counts: Vec<u32>,
    // Per-bin sum of linear power, so the gated mean does not quantise.
    power: Vec<f64>,
    total: u64,
}

impl Histogram {
    fn new() -> Self {
        Self { counts: vec![0; HIST_BINS], power: vec![0.0; HIST_BINS], total: 0 }
    }

    fn clear(&mut self) {
        self.counts.fill(0);
        self.power.fill(0.0);
        self.total = 0;
    }

    fn bin(lufs: f64) -> Option<usize> {
        if lufs < HIST_MIN || !lufs.is_finite() {
            return None;
        }
        Some((((lufs.min(HIST_MAX) - HIST_MIN) / HIST_STEP).round() as usize).min(HIST_BINS - 1))
    }

    /// Adds a block with mean K-weighted power `p` (already channel-summed).
    fn add(&mut self, p: f64) {
        let lufs = LUFS_OFFSET + 10.0 * p.max(1e-30).log10();
        if let Some(b) = Self::bin(lufs) {
            self.counts[b] += 1;
            self.power[b] += p;
            self.total += 1;
        }
    }

    /// Mean power of blocks at or above `floor_lufs`, with their count.
    fn mean_above(&self, floor_lufs: f64) -> (f64, u64) {
        let start = Self::bin(floor_lufs).unwrap_or(0);
        let mut sum = 0.0;
        let mut n = 0u64;
        for b in start..HIST_BINS {
            sum += self.power[b];
            n += self.counts[b] as u64;
        }
        if n == 0 { (0.0, 0) } else { (sum / n as f64, n) }
    }

    /// Loudness at the `pct` percentile (0..100) of blocks at or above `floor_lufs`.
    fn percentile_above(&self, floor_lufs: f64, pct: f64) -> f64 {
        let start = Self::bin(floor_lufs).unwrap_or(0);
        let n: u64 = self.counts[start..].iter().map(|c| *c as u64).sum();
        if n == 0 {
            return f64::NEG_INFINITY;
        }
        let target = (pct / 100.0 * (n - 1) as f64).round() as u64;
        let mut seen = 0u64;
        for b in start..HIST_BINS {
            seen += self.counts[b] as u64;
            if seen > target {
                return HIST_MIN + b as f64 * HIST_STEP;
            }
        }
        HIST_MAX
    }
}

/// What a measurement produced. Values are `NEG_INFINITY` when nothing
/// passed the gate (silence).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessReport {
    /// Gated integrated loudness in LUFS.
    pub integrated_lufs: f64,
    /// Loudest 3 s short-term value in LUFS.
    pub short_term_max_lufs: f64,
    /// Loudest 400 ms momentary value in LUFS.
    pub momentary_max_lufs: f64,
    /// Loudness range in LU (EBU Tech 3342).
    pub range_lu: f64,
    /// True peak in dBTP (4× oversampled).
    pub true_peak_dbtp: f64,
    /// Plain sample peak in dBFS.
    pub sample_peak_dbfs: f64,
}

/// A streaming BS.1770-4 meter for stereo, or for any [`Layout`] with
/// [`LoudnessMeter::with_layout`] (surround channels weighted +1.5 dB, the
/// LFE left out).
pub struct LoudnessMeter {
    rate: u32,
    k: KWeighting,
    // Extra K-weighting and true-peak state for channels past the first
    // pair (surround layouts), and the per-channel weights.
    k_more: Vec<KWeighting>,
    tp_more: Vec<TruePeak>,
    weights: Vec<f64>,
    // 100 ms sub-block accumulation.
    sub_len: usize,
    sub_pos: usize,
    sub_sum: [f64; 2],
    // Ring of the last 30 sub-block powers (3 s), channel-summed.
    ring: [f64; 30],
    ring_pos: usize,
    ring_filled: usize,
    integrated: Histogram,
    short_term: Histogram,
    momentary_max: f64,
    short_term_max: f64,
    momentary_now: f64,
    short_term_now: f64,
    tp: TruePeak,
    true_peak: f32,
    sample_peak: f32,
}

impl LoudnessMeter {
    /// A meter at `rate`.
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            k: KWeighting::new(rate),
            k_more: Vec::new(),
            tp_more: Vec::new(),
            weights: vec![1.0, 1.0],
            sub_len: (rate as usize / 10).max(1),
            sub_pos: 0,
            sub_sum: [0.0; 2],
            ring: [0.0; 30],
            ring_pos: 0,
            ring_filled: 0,
            integrated: Histogram::new(),
            short_term: Histogram::new(),
            momentary_max: f64::NEG_INFINITY,
            short_term_max: f64::NEG_INFINITY,
            momentary_now: f64::NEG_INFINITY,
            short_term_now: f64::NEG_INFINITY,
            tp: TruePeak::new(),
            true_peak: 0.0,
            sample_peak: 0.0,
        }
    }

    /// A meter for a multichannel layout, fed with [`LoudnessMeter::process_multi`].
    pub fn with_layout(rate: u32, layout: Layout) -> Self {
        let mut m = Self::new(rate);
        let pairs = layout.channels().div_ceil(2);
        m.k_more = (1..pairs).map(|_| KWeighting::new(rate)).collect();
        m.tp_more = (1..pairs).map(|_| TruePeak::new()).collect();
        m.weights = layout.loudness_weights().to_vec();
        m
    }

    /// Feeds one block of every channel of the meter's layout. The LFE
    /// counts toward the peaks but not the loudness.
    pub fn process_multi(&mut self, buf: &MultiBuf) {
        let ch = buf.channels().min(self.weights.len());
        for i in 0..buf.frames() {
            let mut sum = 0.0;
            for p in 0..ch.div_ceil(2) {
                let a = buf.chan(2 * p)[i];
                let b = if 2 * p + 1 < ch { buf.chan(2 * p + 1)[i] } else { 0.0 };
                self.sample_peak = self.sample_peak.max(a.abs()).max(b.abs());
                let (k, tp) = if p == 0 { (&mut self.k, &mut self.tp) } else { (&mut self.k_more[p - 1], &mut self.tp_more[p - 1]) };
                self.true_peak = self.true_peak.max(tp.tick(a, b));
                let ka = k.tick(0, a) as f64;
                let kb = k.tick(1, b) as f64;
                sum += self.weights[2 * p] * ka * ka;
                if 2 * p + 1 < ch {
                    sum += self.weights[2 * p + 1] * kb * kb;
                }
            }
            self.sub_sum[0] += sum;
            self.sub_pos += 1;
            if self.sub_pos == self.sub_len {
                self.finish_sub_block();
            }
        }
    }

    /// Sample rate the meter was built for.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Starts a new measurement.
    pub fn reset(&mut self) {
        self.k.reset();
        for k in &mut self.k_more {
            k.reset();
        }
        for t in &mut self.tp_more {
            t.reset();
        }
        self.sub_pos = 0;
        self.sub_sum = [0.0; 2];
        self.ring = [0.0; 30];
        self.ring_pos = 0;
        self.ring_filled = 0;
        self.integrated.clear();
        self.short_term.clear();
        self.momentary_max = f64::NEG_INFINITY;
        self.short_term_max = f64::NEG_INFINITY;
        self.momentary_now = f64::NEG_INFINITY;
        self.short_term_now = f64::NEG_INFINITY;
        self.tp.reset();
        self.true_peak = 0.0;
        self.sample_peak = 0.0;
    }

    /// Feeds one stereo block.
    pub fn process(&mut self, left: &[f32], right: &[f32]) {
        for (l, r) in left.iter().zip(right) {
            self.sample_peak = self.sample_peak.max(l.abs()).max(r.abs());
            self.true_peak = self.true_peak.max(self.tp.tick(*l, *r));
            let kl = self.k.tick(0, *l) as f64;
            let kr = self.k.tick(1, *r) as f64;
            self.sub_sum[0] += kl * kl;
            self.sub_sum[1] += kr * kr;
            self.sub_pos += 1;
            if self.sub_pos == self.sub_len {
                self.finish_sub_block();
            }
        }
    }

    fn finish_sub_block(&mut self) {
        let p = (self.sub_sum[0] + self.sub_sum[1]) / self.sub_len as f64;
        self.sub_sum = [0.0; 2];
        self.sub_pos = 0;
        self.ring[self.ring_pos] = p;
        self.ring_pos = (self.ring_pos + 1) % 30;
        self.ring_filled = (self.ring_filled + 1).min(30);
        let mean_last = |n: usize| -> f64 {
            let mut s = 0.0;
            for i in 0..n {
                s += self.ring[(self.ring_pos + 30 - 1 - i) % 30];
            }
            s / n as f64
        };
        if self.ring_filled >= 4 {
            let m = mean_last(4);
            self.momentary_now = LUFS_OFFSET + 10.0 * m.max(1e-30).log10();
            self.momentary_max = self.momentary_max.max(self.momentary_now);
            self.integrated.add(m);
        }
        if self.ring_filled >= 30 {
            let s = mean_last(30);
            self.short_term_now = LUFS_OFFSET + 10.0 * s.max(1e-30).log10();
            self.short_term_max = self.short_term_max.max(self.short_term_now);
            self.short_term.add(s);
        }
    }

    /// Momentary loudness (last 400 ms) in LUFS.
    pub fn momentary_lufs(&self) -> f64 {
        self.momentary_now
    }

    /// Short-term loudness (last 3 s) in LUFS.
    pub fn short_term_lufs(&self) -> f64 {
        self.short_term_now
    }

    /// Gated integrated loudness so far, in LUFS.
    pub fn integrated_lufs(&self) -> f64 {
        let (ungated, n) = self.integrated.mean_above(ABSOLUTE_GATE);
        if n == 0 {
            return f64::NEG_INFINITY;
        }
        let rel = LUFS_OFFSET + 10.0 * ungated.log10() + RELATIVE_GATE;
        let (gated, n2) = self.integrated.mean_above(rel.max(ABSOLUTE_GATE));
        if n2 == 0 { f64::NEG_INFINITY } else { LUFS_OFFSET + 10.0 * gated.log10() }
    }

    /// Loudness range so far, in LU: the spread between the 10th and 95th
    /// percentiles of short-term loudness, gated at −70 LUFS absolute and
    /// −20 LU relative.
    pub fn range_lu(&self) -> f64 {
        let (ungated, n) = self.short_term.mean_above(ABSOLUTE_GATE);
        if n == 0 {
            return 0.0;
        }
        let rel = (LUFS_OFFSET + 10.0 * ungated.log10() - 20.0).max(ABSOLUTE_GATE);
        let lo = self.short_term.percentile_above(rel, 10.0);
        let hi = self.short_term.percentile_above(rel, 95.0);
        (hi - lo).max(0.0)
    }

    /// True peak so far in dBTP.
    pub fn true_peak_dbtp(&self) -> f64 {
        gain_to_db(self.true_peak) as f64
    }

    /// Everything measured so far.
    pub fn report(&self) -> LoudnessReport {
        LoudnessReport {
            integrated_lufs: self.integrated_lufs(),
            short_term_max_lufs: self.short_term_max,
            momentary_max_lufs: self.momentary_max,
            range_lu: self.range_lu(),
            true_peak_dbtp: self.true_peak_dbtp(),
            sample_peak_dbfs: gain_to_db(self.sample_peak) as f64,
        }
    }
}

/// Measures a whole planar stereo signal (allocates only the meter).
pub fn measure(left: &[f32], right: &[f32], rate: u32) -> LoudnessReport {
    let mut m = LoudnessMeter::new(rate);
    m.process(left, right);
    m.report()
}

/// Measures planar channels of `layout`.
pub fn measure_layout(chans: &[Vec<f32>], rate: u32, layout: Layout) -> LoudnessReport {
    let n = chans.first().map_or(0, Vec::len);
    let mut m = LoudnessMeter::with_layout(rate, layout);
    let block = 4096;
    let mut buf = MultiBuf::new(layout.channels(), block);
    let mut at = 0;
    while at < n {
        let k = block.min(n - at);
        buf.set_frames(k);
        for (c, ch) in chans.iter().enumerate().take(layout.channels()) {
            buf.chan_mut(c).copy_from_slice(&ch[at..at + k]);
        }
        m.process_multi(&buf);
        at += k;
    }
    m.report()
}

/// Loudness of a stereo signal measured only inside `spans` (frame ranges,
/// e.g. where dialogue is speaking), for dialogue-gated specifications.
/// The spans are measured back to back.
pub fn measure_within(left: &[f32], right: &[f32], rate: u32, spans: &[(usize, usize)]) -> LoudnessReport {
    let mut m = LoudnessMeter::new(rate);
    for &(a, b) in spans {
        let (a, b) = (a.min(left.len()), b.min(left.len()));
        if b > a {
            m.process(&left[a..b], &right[a..b]);
        }
    }
    m.report()
}

/// Measures a mono signal (as if it played from both speakers).
pub fn measure_mono(mono: &[f32], rate: u32) -> LoudnessReport {
    measure(mono, mono, rate)
}

/// Delivery loudness specifications: what a mix must measure for where it
/// is going. Values are the published targets (integrated loudness and
/// maximum true peak) of each specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoudnessTarget {
    /// −14 LUFS, −1 dBTP: YouTube, Spotify and most music and video
    /// streaming platforms (they turn louder mixes down).
    Streaming,
    /// −16 LUFS, −1 dBTP: podcasts, Apple Music and spoken word.
    Podcast,
    /// −23 LUFS ±0.5 LU, −1 dBTP: broadcast, EBU R 128.
    Broadcast,
    /// −24 LKFS ±2 LU, −2 dBTP: US broadcast, ATSC A/85.
    AtscA85,
    /// −27 LKFS ±2 LU dialogue-gated, −2 dBTP: Netflix (and services that
    /// follow its spec): loudness is measured on speech only.
    Netflix,
    /// −24 LKFS ±2 LU, −2 dBTP: Amazon Prime Video.
    Amazon,
}

impl LoudnessTarget {
    /// All targets, in menu order.
    pub const ALL: [LoudnessTarget; 6] = [LoudnessTarget::Streaming, LoudnessTarget::Podcast, LoudnessTarget::Broadcast, LoudnessTarget::AtscA85, LoudnessTarget::Netflix, LoudnessTarget::Amazon];

    /// The integrated loudness aimed for.
    pub fn lufs(self) -> f64 {
        match self {
            LoudnessTarget::Streaming => -14.0,
            LoudnessTarget::Podcast => -16.0,
            LoudnessTarget::Broadcast => -23.0,
            LoudnessTarget::AtscA85 | LoudnessTarget::Amazon => -24.0,
            LoudnessTarget::Netflix => -27.0,
        }
    }

    /// The true-peak ceiling that goes with the target, in dBTP.
    pub fn ceiling_dbtp(self) -> f64 {
        match self {
            LoudnessTarget::Streaming | LoudnessTarget::Podcast | LoudnessTarget::Broadcast => -1.0,
            LoudnessTarget::AtscA85 | LoudnessTarget::Netflix | LoudnessTarget::Amazon => -2.0,
        }
    }

    /// How far from the target a mix may measure and still pass, in LU.
    pub fn tolerance_lu(self) -> f64 {
        match self {
            LoudnessTarget::Broadcast => 0.5,
            LoudnessTarget::Streaming | LoudnessTarget::Podcast => 1.0,
            LoudnessTarget::AtscA85 | LoudnessTarget::Netflix | LoudnessTarget::Amazon => 2.0,
        }
    }

    /// Whether loudness is measured on dialogue only.
    pub fn dialogue_gated(self) -> bool {
        self == LoudnessTarget::Netflix
    }

    /// Stable id for project files and the AI.
    pub fn id(self) -> &'static str {
        match self {
            LoudnessTarget::Streaming => "streaming",
            LoudnessTarget::Podcast => "podcast",
            LoudnessTarget::Broadcast => "ebu_r128",
            LoudnessTarget::AtscA85 => "atsc_a85",
            LoudnessTarget::Netflix => "netflix",
            LoudnessTarget::Amazon => "amazon",
        }
    }

    /// Parses an id or a common name (`r128`, `ebu`, `broadcast`, `atsc`,
    /// `a85`, `youtube`, `spotify`, `apple`, `prime`…).
    pub fn from_id(s: &str) -> Option<LoudnessTarget> {
        let k = s.trim().to_ascii_lowercase().replace(['-', ' ', '/', '.'], "_");
        Some(match k.as_str() {
            "streaming" | "youtube" | "spotify" | "tiktok" | "instagram" | "social" | "web" => LoudnessTarget::Streaming,
            "podcast" | "apple_music" | "apple" | "spoken_word" => LoudnessTarget::Podcast,
            "ebu_r128" | "ebu_r_128" | "r128" | "ebu" | "broadcast" | "ebu_r_128_" => LoudnessTarget::Broadcast,
            "atsc_a85" | "atsc" | "a85" | "atsc_a_85" | "us_broadcast" | "calm" => LoudnessTarget::AtscA85,
            "netflix" | "disney" | "disney_plus" | "hbo" | "max" => LoudnessTarget::Netflix,
            "amazon" | "prime" | "prime_video" | "amazon_prime" => LoudnessTarget::Amazon,
            _ => return None,
        })
    }

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            LoudnessTarget::Streaming => "Streaming (−14 LUFS)",
            LoudnessTarget::Podcast => "Podcast (−16 LUFS)",
            LoudnessTarget::Broadcast => "Broadcast EBU R128 (−23 LUFS)",
            LoudnessTarget::AtscA85 => "US broadcast ATSC A/85 (−24 LKFS)",
            LoudnessTarget::Netflix => "Netflix (−27 LKFS dialogue)",
            LoudnessTarget::Amazon => "Amazon Prime Video (−24 LKFS)",
        }
    }
}

/// The gain in dB that brings `report` to `target_lufs`. When that gain would
/// push the true peak over `ceiling_dbtp`, the gain is reduced so the peak
/// sits at the ceiling (the caller may follow with a limiter instead by
/// passing `f64::INFINITY`). Silence returns 0.
pub fn normalise_gain_db(report: &LoudnessReport, target_lufs: f64, ceiling_dbtp: f64) -> f32 {
    if !report.integrated_lufs.is_finite() {
        return 0.0;
    }
    let mut gain = target_lufs - report.integrated_lufs;
    if report.true_peak_dbtp.is_finite() && report.true_peak_dbtp + gain > ceiling_dbtp {
        gain = ceiling_dbtp - report.true_peak_dbtp;
    }
    gain as f32
}

/// Test signals in the spirit of EBU Tech 3341's compliance set, generated
/// at any rate: each is `(left, right, expected integrated LUFS)`.
pub fn test_vectors(rate: u32) -> Vec<(Vec<f32>, Vec<f32>, f64)> {
    let tone = |db: f64, secs: f64| -> Vec<f32> {
        let amp = 10f64.powf(db / 20.0);
        let n = (secs * rate as f64) as usize;
        (0..n).map(|i| (amp * (std::f64::consts::TAU * 1000.0 * i as f64 / rate as f64).sin()) as f32).collect()
    };
    let mut out = Vec::new();
    // 1. Steady −23 dBFS stereo tone reads −23 LUFS.
    let a = tone(-23.0, 20.0);
    out.push((a.clone(), a, -23.0));
    // 2. Steady −33 dBFS reads −33 LUFS.
    let b = tone(-33.0, 20.0);
    out.push((b.clone(), b, -33.0));
    // 3. Relative gate: quiet −38 dBFS passages around a −23 dBFS section
    //    are gated out (the ungated mean is about −26, so the gate sits near
    //    −36), and the result is −23 LUFS.
    let mut c = tone(-38.0, 10.0);
    c.extend(tone(-23.0, 20.0));
    c.extend(tone(-38.0, 10.0));
    out.push((c.clone(), c, -23.0));
    // 4. Absolute gate: a −23 dBFS section with near-silence (−90 dBFS)
    //    around it also reads −23 LUFS.
    let mut d = tone(-90.0, 10.0);
    d.extend(tone(-23.0, 20.0));
    d.extend(tone(-90.0, 10.0));
    out.push((d.clone(), d, -23.0));
    // 5. Relative gate with a run of −30 dBFS: still gated (7 LU under).
    let mut e = tone(-30.0, 20.0);
    e.extend(tone(-23.0, 20.0));
    e.extend(tone(-30.0, 20.0));
    // Ungated mean of 1/3 at −23 and 2/3 at −30: 10·log10((1·10^-2.3 + 2·10^-3)/3) = −27.06;
    // relative gate at −37.06 keeps everything, so the result is the ungated mean.
    let expect = 10.0 * ((10f64.powf(-2.3) + 2.0 * 10f64.powf(-3.0)) / 3.0).log10();
    out.push((e.clone(), e, expect));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k_weighting_matches_the_published_48k_coefficients() {
        let s = KWeighting::shelf_coeffs(48_000);
        let want = [1.535_124_859_586_97, -2.691_696_189_406_38, 1.198_392_810_852_85, -1.690_659_293_182_41, 0.732_480_774_215_85];
        for (got, w) in [s.b0, s.b1, s.b2, s.a1, s.a2].iter().zip(want) {
            assert!((*got as f64 - w).abs() < 2e-6, "{got} vs {w}");
        }
        let h = KWeighting::high_pass_coeffs(48_000);
        let want = [1.0, -2.0, 1.0, -1.990_047_454_833_98, 0.990_072_250_366_21];
        for (got, w) in [h.b0, h.b1, h.b2, h.a1, h.a2].iter().zip(want) {
            assert!((*got as f64 - w).abs() < 2e-6, "{got} vs {w}");
        }
    }

    #[test]
    fn test_vectors_read_their_expected_loudness() {
        for rate in [48_000, 44_100] {
            for (i, (l, r, want)) in test_vectors(rate).into_iter().enumerate() {
                let rep = measure(&l, &r, rate);
                assert!((rep.integrated_lufs - want).abs() < 0.1, "vector {i} at {rate}: {} vs {want}", rep.integrated_lufs);
            }
        }
    }

    #[test]
    fn streaming_in_blocks_matches_whole_and_tracks_short_term() {
        let rate = 48_000;
        let (l, r, want) = test_vectors(rate).swap_remove(2);
        let whole = measure(&l, &r, rate);
        let mut m = LoudnessMeter::new(rate);
        let mut st_at_15s = 0.0;
        for (i, (a, b)) in l.chunks(333).zip(r.chunks(333)).enumerate() {
            m.process(a, b);
            if i * 333 <= 15 * rate as usize && (i + 1) * 333 > 15 * rate as usize {
                st_at_15s = m.short_term_lufs();
            }
        }
        let rep = m.report();
        assert!((rep.integrated_lufs - whole.integrated_lufs).abs() < 1e-6);
        assert!((rep.integrated_lufs - want).abs() < 0.1);
        assert!((st_at_15s + 23.0).abs() < 0.1, "{st_at_15s}");
        assert!((rep.momentary_max_lufs + 23.0).abs() < 0.1);
        // Range: −38 and −23 sections; the −20 LU relative gate (near −46)
        // keeps both, so the range is about their 15 LU spread.
        assert!(rep.range_lu > 12.0 && rep.range_lu < 16.0, "{}", rep.range_lu);
        // A −23 dBFS sine has a true peak of −23 dBTP.
        assert!((rep.true_peak_dbtp + 23.0).abs() < 0.1, "{}", rep.true_peak_dbtp);
    }

    #[test]
    fn silence_is_minus_infinity_and_normalise_respects_ceiling() {
        let z = vec![0.0f32; 48_000 * 2];
        let rep = measure(&z, &z, 48_000);
        assert_eq!(rep.integrated_lufs, f64::NEG_INFINITY);
        assert_eq!(normalise_gain_db(&rep, -14.0, -1.0), 0.0);
        let rep = LoudnessReport { integrated_lufs: -20.0, short_term_max_lufs: -18.0, momentary_max_lufs: -16.0, range_lu: 5.0, true_peak_dbtp: -3.0, sample_peak_dbfs: -3.2 };
        assert!((normalise_gain_db(&rep, -16.0, -1.0) - 2.0).abs() < 1e-6);
        assert!((normalise_gain_db(&rep, -14.0, -1.0) - 2.0).abs() < 1e-6, "ceiling caps it");
        assert!((normalise_gain_db(&rep, -14.0, f64::INFINITY) - 6.0).abs() < 1e-6);
        assert_eq!(LoudnessTarget::Podcast.lufs(), -16.0);
    }

    #[test]
    fn k_weighting_is_flat_at_1khz_and_rolls_off_bass() {
        let k = KWeighting::new(48_000);
        let at = |f: f32| k.shelf.coeffs.response_db(48_000, f) + k.high_pass.coeffs.response_db(48_000, f);
        assert!((at(1000.0) - 0.691).abs() < 0.05, "{}", at(1000.0));
        assert!(at(10_000.0) > 3.5);
        assert!(at(30.0) < -3.0);
    }
}
