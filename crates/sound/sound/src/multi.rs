// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Multichannel blocks for buses and the master of a surround mix:
//! [`MultiBuf`], effect chains run per channel pair ([`MultiFx`]) and a
//! linked look-ahead true-peak limiter over every channel ([`MultiLimiter`]).
//! Like the rest of the crate, everything is sized at construction and never
//! allocates while processing.

use crate::buffer::{Effect, db_to_gain, gain_to_db};
use crate::dynamics::{LimiterParams, TruePeak, time_coeff};
use crate::mixer::EffectSpec;
use crate::surround::{CENTRE, LFE, Layout};

/// Planar block of N channels with a fixed capacity.
#[derive(Clone, Debug)]
pub struct MultiBuf {
    chans: Vec<Vec<f32>>,
    frames: usize,
}

impl MultiBuf {
    /// `channels` silent channels of `capacity` frames.
    pub fn new(channels: usize, capacity: usize) -> Self {
        Self { chans: vec![vec![0.0; capacity]; channels.max(1)], frames: capacity }
    }

    /// Number of channels.
    pub fn channels(&self) -> usize {
        self.chans.len()
    }

    /// Active block length.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Sets the block length (panics above capacity).
    pub fn set_frames(&mut self, n: usize) {
        assert!(n <= self.chans[0].len(), "block of {n} frames exceeds capacity");
        self.frames = n;
    }

    /// Zeroes the active block.
    pub fn clear(&mut self) {
        let n = self.frames;
        for c in &mut self.chans {
            c[..n].fill(0.0);
        }
    }

    /// One channel of the active block.
    pub fn chan(&self, i: usize) -> &[f32] {
        &self.chans[i][..self.frames]
    }

    /// One channel of the active block, mutably.
    pub fn chan_mut(&mut self, i: usize) -> &mut [f32] {
        let n = self.frames;
        &mut self.chans[i][..n]
    }

    /// Two different channels mutably at once.
    pub fn pair_mut(&mut self, a: usize, b: usize) -> (&mut [f32], &mut [f32]) {
        assert!(a != b);
        let n = self.frames;
        if a < b {
            let (lo, hi) = self.chans.split_at_mut(b);
            (&mut lo[a][..n], &mut hi[0][..n])
        } else {
            let (lo, hi) = self.chans.split_at_mut(a);
            (&mut hi[0][..n], &mut lo[b][..n])
        }
    }

    /// Adds `other` (same channel count) into this block.
    pub fn add(&mut self, other: &MultiBuf) {
        let n = self.frames.min(other.frames);
        for (c, o) in self.chans.iter_mut().zip(&other.chans) {
            for i in 0..n {
                c[i] += o[i];
            }
        }
    }

    /// Multiplies every channel by `g`.
    pub fn scale(&mut self, g: f32) {
        let n = self.frames;
        for c in &mut self.chans {
            for v in &mut c[..n] {
                *v *= g;
            }
        }
    }

    /// Multiplies every channel by a per-frame gain.
    pub fn apply_gain(&mut self, gains: &[f32]) {
        let n = self.frames.min(gains.len());
        for c in &mut self.chans {
            for i in 0..n {
                c[i] *= gains[i];
            }
        }
    }

    /// Largest absolute sample over all channels.
    pub fn peak(&self) -> f32 {
        self.chans.iter().map(|c| c[..self.frames].iter().fold(0.0f32, |m, v| m.max(v.abs()))).fold(0.0, f32::max)
    }

    /// Largest absolute sample of one channel.
    pub fn chan_peak(&self, i: usize) -> f32 {
        self.chans[i][..self.frames].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }
}

/// The channel groups effects run on for a layout: stereo pairs, the centre
/// on its own, and the LFE left out (bus EQ and compression are not for
/// the sub channel).
pub fn effect_groups(layout: Layout) -> &'static [(usize, Option<usize>)] {
    match layout {
        Layout::Stereo => &[(0, Some(1))],
        Layout::Surround51 => &[(0, Some(1)), (CENTRE, None), (4, Some(5))],
        Layout::Surround71 => &[(0, Some(1)), (CENTRE, None), (4, Some(5)), (6, Some(7))],
    }
}

/// One effect chain per channel group of a layout.
pub struct MultiFx {
    layout: Layout,
    chains: Vec<Vec<Box<dyn Effect>>>,
    scratch: Vec<f32>,
}

impl MultiFx {
    /// Builds `specs` once per channel group. Allocates.
    pub fn new(specs: &[EffectSpec], layout: Layout, rate: u32, max_block: usize) -> Self {
        let chains = effect_groups(layout).iter().map(|_| specs.iter().map(|s| s.build(rate, max_block)).collect()).collect();
        Self { layout, chains, scratch: vec![0.0; max_block] }
    }

    /// Whether there is nothing to run.
    pub fn is_empty(&self) -> bool {
        self.chains.first().is_none_or(Vec::is_empty)
    }

    /// Runs the chains in place.
    pub fn process(&mut self, buf: &mut MultiBuf) {
        if self.is_empty() {
            return;
        }
        let n = buf.frames();
        for ((a, b), chain) in effect_groups(self.layout).iter().zip(&mut self.chains) {
            match b {
                Some(b) => {
                    let (l, r) = buf.pair_mut(*a, *b);
                    for e in chain.iter_mut() {
                        e.process(l, r);
                    }
                }
                None => {
                    let s = &mut self.scratch[..n];
                    s.copy_from_slice(buf.chan(*a));
                    let c = buf.chan_mut(*a);
                    for e in chain.iter_mut() {
                        e.process(c, s);
                    }
                }
            }
        }
    }

    /// Forgets signal history.
    pub fn reset(&mut self) {
        for c in &mut self.chains {
            for e in c {
                e.reset();
            }
        }
    }

    /// Latency of one chain in frames.
    pub fn latency(&self) -> usize {
        self.chains.first().map(|c| c.iter().map(|e| e.latency()).sum()).unwrap_or(0)
    }

    /// Longest tail in frames.
    pub fn tail(&self) -> usize {
        self.chains.first().map(|c| c.iter().map(|e| e.tail().max(e.latency())).max().unwrap_or(0)).unwrap_or(0)
    }
}

const LOOKAHEAD_MS: f32 = 5.0;
const TP_TAPS_MIN: usize = 12;

/// Look-ahead brick-wall limiter linked over N channels (one gain for all,
/// so the surround image never shifts), with true-peak detection per
/// channel pair. Same design as [`crate::dynamics::Limiter`].
pub struct MultiLimiter {
    params: LimiterParams,
    lookahead: usize,
    delay: Vec<Vec<f32>>,
    write: usize,
    deque_idx: Vec<usize>,
    deque_val: Vec<f32>,
    dq_head: usize,
    dq_tail: usize,
    dq_len: usize,
    min_hist: Vec<f32>,
    min_sum: f64,
    n: usize,
    tps: Vec<TruePeak>,
    release: f32,
    gr_smooth: f32,
    last_gr: f32,
    frame: Vec<f32>,
}

impl MultiLimiter {
    /// A limiter for `channels` at `rate`.
    pub fn new(rate: u32, channels: usize, params: LimiterParams) -> Self {
        let lookahead = ((LOOKAHEAD_MS * 0.001 * rate as f32) as usize).max(TP_TAPS_MIN);
        let cap = lookahead + 1;
        let mut l = Self {
            params,
            lookahead,
            delay: vec![vec![0.0; cap]; channels],
            write: 0,
            deque_idx: vec![0; cap + 1],
            deque_val: vec![1.0; cap + 1],
            dq_head: 0,
            dq_tail: 0,
            dq_len: 0,
            min_hist: vec![1.0; lookahead],
            min_sum: lookahead as f64,
            n: 0,
            tps: (0..channels.div_ceil(2)).map(|_| TruePeak::new()).collect(),
            release: 0.0,
            gr_smooth: 0.0,
            last_gr: 0.0,
            frame: vec![0.0; channels],
        };
        l.set_params(params, rate);
        l
    }

    /// New settings.
    pub fn set_params(&mut self, p: LimiterParams, rate: u32) {
        self.params = p;
        self.release = time_coeff(p.release_ms.max(1.0), rate);
    }

    /// Gain reduction at the end of the last block, dB.
    pub fn gain_reduction_db(&self) -> f32 {
        self.last_gr
    }

    /// Delay in frames.
    pub fn latency(&self) -> usize {
        self.lookahead
    }

    fn deque_push(&mut self, idx: usize, val: f32) {
        let len = self.deque_idx.len();
        while self.dq_len > 0 {
            let back = (self.dq_tail + len - 1) % len;
            if self.deque_val[back] >= val {
                self.dq_tail = back;
                self.dq_len -= 1;
            } else {
                break;
            }
        }
        self.deque_idx[self.dq_tail] = idx;
        self.deque_val[self.dq_tail] = val;
        self.dq_tail = (self.dq_tail + 1) % len;
        self.dq_len += 1;
    }

    /// Limits the block in place.
    pub fn process(&mut self, buf: &mut MultiBuf) {
        let ceiling = db_to_gain(self.params.ceiling_db);
        let ch = self.delay.len().min(buf.channels());
        let cap = self.delay[0].len();
        for i in 0..buf.frames() {
            for c in 0..ch {
                self.frame[c] = buf.chan(c)[i];
            }
            let mut peak = 0.0f32;
            if self.params.true_peak {
                for (p, tp) in self.tps.iter_mut().enumerate() {
                    let a = self.frame[2 * p];
                    let b = if 2 * p + 1 < ch { self.frame[2 * p + 1] } else { 0.0 };
                    peak = peak.max(tp.tick(a, b));
                }
            } else {
                peak = self.frame[..ch].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            }
            let needed_db = if peak > ceiling { gain_to_db(peak / ceiling) } else { 0.0 };
            self.gr_smooth = if needed_db > self.gr_smooth { needed_db } else { needed_db + self.release * (self.gr_smooth - needed_db) };
            let g = db_to_gain(-self.gr_smooth);
            self.deque_push(self.n, g);
            let oldest = self.n.saturating_sub(self.lookahead);
            while self.dq_len > 0 && self.deque_idx[self.dq_head] < oldest {
                self.dq_head = (self.dq_head + 1) % self.deque_idx.len();
                self.dq_len -= 1;
            }
            let g_min = self.deque_val[self.dq_head];
            let slot = self.n % self.lookahead;
            self.min_sum += g_min as f64 - self.min_hist[slot] as f64;
            self.min_hist[slot] = g_min;
            let g_apply = (self.min_sum / self.lookahead as f64) as f32;
            let read = (self.write + cap - self.lookahead) % cap;
            for c in 0..ch {
                let d = self.delay[c][read];
                self.delay[c][self.write] = self.frame[c];
                buf.chan_mut(c)[i] = d * g_apply;
            }
            self.write = (self.write + 1) % cap;
            self.n += 1;
            self.last_gr = -gain_to_db(g_apply.max(1e-9));
        }
    }

    /// Forgets history.
    pub fn reset(&mut self) {
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
        for t in &mut self.tps {
            t.reset();
        }
        self.gr_smooth = 0.0;
        self.last_gr = 0.0;
    }
}

/// Second-order Butterworth low-pass run twice (24 dB/octave), for the LFE
/// channel's 120 Hz band limit.
pub struct LfeFilter {
    a: crate::eq::Biquad,
    b: crate::eq::Biquad,
}

impl LfeFilter {
    /// The filter at `rate` with corner `hz`.
    pub fn new(rate: u32, hz: f32) -> Self {
        use crate::eq::{BandKind, Biquad};
        let q = std::f32::consts::FRAC_1_SQRT_2;
        Self { a: Biquad::design(BandKind::LowPass, rate, hz, q, 0.0), b: Biquad::design(BandKind::LowPass, rate, hz, q, 0.0) }
    }

    /// Filters the LFE channel of `buf` (no-op for stereo).
    pub fn process(&mut self, buf: &mut MultiBuf) {
        if buf.channels() <= LFE {
            return;
        }
        let c = buf.chan_mut(LFE);
        self.a.process_channel(0, c);
        self.b.process_channel(0, c);
    }

    /// Forgets history.
    pub fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_mut_gives_the_right_channels() {
        let mut b = MultiBuf::new(6, 4);
        b.chan_mut(4).fill(4.0);
        b.chan_mut(1).fill(1.0);
        let (x, y) = b.pair_mut(4, 1);
        assert_eq!((x[0], y[0]), (4.0, 1.0));
        assert_eq!(b.peak(), 4.0);
        assert_eq!(b.chan_peak(1), 1.0);
    }

    #[test]
    fn the_limiter_holds_every_channel_under_the_ceiling_with_one_gain() {
        let rate = 48_000;
        let mut lim = MultiLimiter::new(rate, 6, LimiterParams { ceiling_db: -1.0, ..Default::default() });
        let n = 4800;
        let mut b = MultiBuf::new(6, n);
        for i in 0..n {
            let s = (i as f32 * 0.05).sin();
            b.chan_mut(0)[i] = s * 2.0;
            b.chan_mut(4)[i] = s * 0.5;
        }
        lim.process(&mut b);
        let ceiling = db_to_gain(-1.0) * 1.02;
        assert!(b.peak() <= ceiling, "peak {}", b.peak());
        // Linked: the quiet channel is reduced by the same factor.
        let late = 3000..4700;
        let r0: f32 = late.clone().map(|i| b.chan(0)[i].abs()).fold(0.0, f32::max);
        let r4: f32 = late.map(|i| b.chan(4)[i].abs()).fold(0.0, f32::max);
        assert!((r0 / r4 - 4.0).abs() < 0.1, "{r0} / {r4}");
        assert_eq!(lim.latency(), 240);
    }

    #[test]
    fn effects_run_per_group_and_skip_the_lfe() {
        let spec = EffectSpec::Compressor(crate::dynamics::CompressorParams { threshold_db: -40.0, ratio: 10.0, ..Default::default() });
        let mut fx = MultiFx::new(&[spec], Layout::Surround51, 48_000, 2048);
        let mut b = MultiBuf::new(6, 2048);
        for c in 0..6 {
            b.chan_mut(c).fill(0.5);
        }
        for _ in 0..20 {
            fx.process(&mut b);
            for c in 0..6 {
                if c != LFE {
                    b.chan_mut(c).fill(0.5);
                }
            }
        }
        fx.process(&mut b);
        assert_eq!(b.chan(LFE)[100], 0.5);
        assert!(b.chan(0)[2000] < 0.2 && b.chan(CENTRE)[2000] < 0.2, "{} {}", b.chan(0)[2000], b.chan(CENTRE)[2000]);
    }

    #[test]
    fn lfe_filter_removes_highs() {
        let rate = 48_000;
        let mut f = LfeFilter::new(rate, 120.0);
        let n = 9600;
        let mut b = MultiBuf::new(6, n);
        for i in 0..n {
            b.chan_mut(LFE)[i] = (std::f32::consts::TAU * 1000.0 * i as f32 / rate as f32).sin();
        }
        f.process(&mut b);
        assert!(b.chan(LFE)[4800..].iter().fold(0.0f32, |m, v| m.max(v.abs())) < 0.01);
    }
}
