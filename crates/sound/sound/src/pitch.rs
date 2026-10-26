// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Time stretching and pitch shifting.
//!
//! [`Stretcher`] is a streaming WSOLA (waveform-similarity overlap-add) time
//! stretcher: it slides a Hann-windowed grain along the input at the rate the
//! stretch factor asks for, but lets each grain start within a small
//! tolerance of its nominal position at the offset that best matches the
//! waveform continuing from the previous grain, so overlap-add does not
//! cancel. That keeps pitch and timbre intact while speed changes.
//!
//! [`PitchShifter`] combines a [`Resampler`]
//! with a `Stretcher` so output length equals input length and only the
//! pitch moves; it has a fixed latency and is allocation-free per block.

use crate::resample::{Quality, Resampler};

/// Window length in milliseconds; a compromise between pitched-voice
/// continuity (longer) and transient smearing (shorter).
const WINDOW_MS: f32 = 40.0;
const TOLERANCE_MS: f32 = 8.0;

/// Streaming WSOLA time stretcher for stereo.
pub struct Stretcher {
    factor: f64,
    window: usize,
    hop: usize,
    tolerance: usize,
    win: Vec<f32>,
    // Input ring (planar) with absolute frame numbering.
    input: [Vec<f32>; 2],
    in_start: u64,
    in_len: usize,
    in_consumed_total: u64,
    // Nominal analysis position (absolute input frame) for the next grain.
    analysis_pos: f64,
    // The frame after the end of the last grain actually used, for matching.
    last_offset: Option<u64>,
    // Output overlap-add buffer: `window` frames; the first `hop` are complete.
    ola: [Vec<f32>; 2],
    ola_ready: usize,
    out_read: usize,
    mono: Vec<f32>,
    primed: bool,
}

impl Stretcher {
    /// A stretcher at `rate`. `factor` is output length / input length
    /// (2.0 = half speed, 0.5 = double speed). `max_block` bounds a `push`.
    pub fn new(rate: u32, factor: f64, max_block: usize) -> Self {
        let window = ((WINDOW_MS * 0.001 * rate as f32) as usize).max(64) & !1;
        let hop = window / 2;
        let tolerance = (TOLERANCE_MS * 0.001 * rate as f32) as usize;
        let mut win = vec![0.0; window];
        crate::fft::hann(&mut win);
        // Enough input to hold the search region plus a few blocks.
        let cap = window * 4 + tolerance * 2 + max_block.max(1) * 2 + (hop as f64 * 8.0 / factor.clamp(0.1, 10.0)) as usize;
        Self {
            factor: factor.clamp(0.1, 10.0),
            window,
            hop,
            tolerance,
            win,
            input: [vec![0.0; cap], vec![0.0; cap]],
            in_start: 0,
            in_len: 0,
            in_consumed_total: 0,
            analysis_pos: 0.0,
            last_offset: None,
            ola: [vec![0.0; window], vec![0.0; window]],
            ola_ready: 0,
            out_read: 0,
            mono: vec![0.0; window],
            primed: false,
        }
    }

    /// Current stretch factor.
    pub fn factor(&self) -> f64 {
        self.factor
    }

    /// Changes the factor for following grains (speed ramps).
    pub fn set_factor(&mut self, factor: f64) {
        self.factor = factor.clamp(0.1, 10.0);
    }

    /// Frames of input the stretcher can accept right now.
    pub fn space(&self) -> usize {
        self.input[0].len() - self.in_len
    }

    /// Frames of input to push before [`Stretcher::pull`] can produce more.
    /// Zero means a pull will produce something.
    pub fn needed_input(&self) -> usize {
        let need_end = self.analysis_pos as u64 + self.window as u64 + self.tolerance as u64 + 1;
        let have_end = self.in_start + self.in_len as u64;
        need_end.saturating_sub(have_end) as usize
    }

    /// Appends input frames (at most [`Stretcher::space`]; the rest is dropped).
    pub fn push(&mut self, left: &[f32], right: &[f32]) -> usize {
        let n = left.len().min(right.len()).min(self.space());
        let cap = self.input[0].len();
        let start = (self.in_start as usize + self.in_len) % cap;
        for i in 0..n {
            let at = (start + i) % cap;
            self.input[0][at] = left[i];
            self.input[1][at] = right[i];
        }
        self.in_len += n;
        n
    }

    #[inline]
    fn sample(&self, ch: usize, abs: u64) -> f32 {
        if abs < self.in_start || abs >= self.in_start + self.in_len as u64 {
            return 0.0;
        }
        let cap = self.input[ch].len();
        self.input[ch][(abs as usize) % cap]
    }

    /// Latency in output frames before the first grain is complete.
    pub fn latency(&self) -> usize {
        self.hop
    }

    /// Produces up to `out_l.len()` frames, returning how many were written.
    pub fn pull(&mut self, out_l: &mut [f32], out_r: &mut [f32]) -> usize {
        let n = out_l.len().min(out_r.len());
        let mut written = 0;
        while written < n {
            if self.out_read < self.ola_ready {
                let take = (self.ola_ready - self.out_read).min(n - written);
                out_l[written..written + take].copy_from_slice(&self.ola[0][self.out_read..self.out_read + take]);
                out_r[written..written + take].copy_from_slice(&self.ola[1][self.out_read..self.out_read + take]);
                self.out_read += take;
                written += take;
                continue;
            }
            if self.needed_input() > 0 {
                break;
            }
            self.next_grain();
        }
        written
    }

    /// Picks the next grain, overlap-adds it, and releases `hop` frames.
    fn next_grain(&mut self) {
        let nominal = self.analysis_pos.round() as i64;
        let lo = (nominal - self.tolerance as i64).max(self.in_start as i64) as u64;
        let hi = (nominal + self.tolerance as i64) as u64;
        // Best offset: maximise normalised cross-correlation between the
        // candidate grain and the natural continuation of the last grain.
        let chosen = match self.last_offset {
            None => nominal.max(self.in_start as i64) as u64,
            Some(cont) => {
                let mut best = (f32::MIN, nominal.max(lo as i64) as u64);
                // Cache the continuation as mono.
                for i in 0..self.window {
                    self.mono[i] = 0.5 * (self.sample(0, cont + i as u64) + self.sample(1, cont + i as u64));
                }
                let mut off = lo;
                while off <= hi {
                    let mut dot = 0.0f32;
                    let mut e = 1e-9f32;
                    let mut i = 0;
                    while i < self.window {
                        let s = 0.5 * (self.sample(0, off + i as u64) + self.sample(1, off + i as u64));
                        dot += s * self.mono[i];
                        e += s * s;
                        i += 2; // every other sample is enough for the search
                    }
                    let score = dot / e.sqrt();
                    if score > best.0 {
                        best = (score, off);
                    }
                    off += 1;
                }
                best.1
            }
        };
        // Shift the OLA buffer by hop, then add the new windowed grain.
        for ch in 0..2 {
            self.ola[ch].copy_within(self.hop.., 0);
            self.ola[ch][self.window - self.hop..].fill(0.0);
            for i in 0..self.window {
                self.ola[ch][i] += self.sample(ch, chosen + i as u64) * self.win[i];
            }
        }
        self.last_offset = Some(chosen + self.hop as u64);
        self.analysis_pos += self.hop as f64 / self.factor;
        // Release: the first `hop` frames are now complete. The very first
        // grain's first half has only one window on it; emit it anyway (it is
        // the attack of the clip, faded in by the window).
        if self.primed {
            self.ola_ready = self.hop;
        } else {
            self.ola_ready = self.hop;
            self.primed = true;
        }
        self.out_read = 0;
        // Drop input no grain can need any more.
        let keep_from = (self.analysis_pos as i64 - self.tolerance as i64 - 2).max(0) as u64;
        let keep_from = keep_from.min(self.last_offset.unwrap_or(keep_from));
        if keep_from > self.in_start {
            let drop = (keep_from - self.in_start) as usize;
            let drop = drop.min(self.in_len);
            self.in_start += drop as u64;
            self.in_len -= drop;
            self.in_consumed_total += drop as u64;
        }
    }

    /// Forgets everything (on seek).
    pub fn reset(&mut self) {
        self.in_start = 0;
        self.in_len = 0;
        self.in_consumed_total = 0;
        self.analysis_pos = 0.0;
        self.last_offset = None;
        for o in &mut self.ola {
            o.fill(0.0);
        }
        self.ola_ready = 0;
        self.out_read = 0;
        self.primed = false;
    }
}

/// Offline time stretch of planar stereo by `factor` (output/input length).
/// Allocates; speed changes with pitch preservation use this at render time
/// through the mixer's source layer, or directly for conforming.
pub fn stretch(left: &[f32], right: &[f32], rate: u32, factor: f64) -> (Vec<f32>, Vec<f32>) {
    let n = left.len().min(right.len());
    let want = (n as f64 * factor).round() as usize;
    let block = 1024;
    let mut st = Stretcher::new(rate, factor, block);
    let mut out_l = Vec::with_capacity(want + st.window);
    let mut out_r = Vec::with_capacity(want + st.window);
    let (mut tl, mut tr) = (vec![0.0; block], vec![0.0; block]);
    let mut i = 0;
    while out_l.len() < want + st.hop {
        if st.needed_input() > 0 || i < n && st.space() > 0 {
            if i < n {
                let m = (n - i).min(block).min(st.space());
                if m == 0 {
                    break;
                }
                st.push(&left[i..i + m], &right[i..i + m]);
                i += m;
            } else {
                // Pad silence at the end so the tail grains complete.
                let zeros = vec![0.0; block];
                if st.space() == 0 {
                    break;
                }
                st.push(&zeros, &zeros);
            }
        }
        let k = st.pull(&mut tl, &mut tr);
        out_l.extend_from_slice(&tl[..k]);
        out_r.extend_from_slice(&tr[..k]);
        if k == 0 && i >= n && st.space() == 0 {
            break;
        }
    }
    // The first grain starts at input 0 with the window's rising half, so the
    // output is aligned to the input; just trim to length.
    out_l.truncate(want);
    out_r.truncate(want);
    out_l.resize(want, 0.0);
    out_r.resize(want, 0.0);
    (out_l, out_r)
}

/// Streaming pitch shifter: same length out as in, pitch moved by semitones.
pub struct PitchShifter {
    rate: u32,
    semitones: f32,
    resampler: Resampler,
    stretcher: Stretcher,
    // Resampler output scratch.
    rs_l: Vec<f32>,
    rs_r: Vec<f32>,
    // Output FIFO.
    fifo: [Vec<f32>; 2],
    fifo_len: usize,
    max_block: usize,
    latency: usize,
}

impl PitchShifter {
    /// Builds a shifter at `rate` for blocks up to `max_block`. Range is
    /// ±24 semitones.
    pub fn new(rate: u32, semitones: f32, max_block: usize) -> Self {
        let semis = semitones.clamp(-24.0, 24.0);
        let ratio = 2f64.powf(semis as f64 / 12.0);
        let max_block = max_block.max(32);
        let resampler = Resampler::with_ratio(ratio, Quality::Good, max_block);
        let stretcher = Stretcher::new(rate, ratio, resampler.max_output(max_block) + 8);
        let rs_cap = resampler.max_output(max_block).max((max_block as f64 * 4.0) as usize + 8);
        // Latency: the stretcher needs a window plus its search tolerance of
        // input before its first grain, the resampler looks ahead half a
        // kernel, and a block of slack covers rounding of the output counts.
        // The FIFO starts holding this many zeros and must never run dry.
        let latency = stretcher.window + stretcher.tolerance + resampler.latency() + max_block + 64;
        let fifo_cap = latency + max_block * 6 + stretcher.window * 2;
        let mut p = Self {
            rate,
            semitones: semis,
            resampler,
            stretcher,
            rs_l: vec![0.0; rs_cap],
            rs_r: vec![0.0; rs_cap],
            fifo: [vec![0.0; fifo_cap], vec![0.0; fifo_cap]],
            fifo_len: 0,
            max_block,
            latency,
        };
        p.fifo_len = p.latency;
        p
    }

    /// Current shift in semitones.
    pub fn semitones(&self) -> f32 {
        self.semitones
    }

    /// Changes the shift; takes effect over the next grains.
    pub fn set_semitones(&mut self, semitones: f32) {
        self.semitones = semitones.clamp(-24.0, 24.0);
        let ratio = 2f64.powf(self.semitones as f64 / 12.0);
        self.resampler.set_ratio(ratio);
        self.stretcher.set_factor(ratio);
    }

    /// Sample rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Frames of delay (constant).
    pub fn latency(&self) -> usize {
        self.latency
    }
}

/// Pulls everything the stretcher has into the FIFO.
fn drain_into(stretcher: &mut Stretcher, fifo: &mut [Vec<f32>; 2], fifo_len: &mut usize) {
    let cap = fifo[0].len();
    if *fifo_len >= cap {
        return;
    }
    let [f0, f1] = fifo;
    let got = stretcher.pull(&mut f0[*fifo_len..cap], &mut f1[*fifo_len..cap]);
    *fifo_len += got;
}

impl crate::buffer::Effect for PitchShifter {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let mut done = 0;
        while done < n {
            let m = (n - done).min(self.max_block);
            // Resample (changes pitch and length), then stretch back to length.
            let k = self.resampler.process(&left[done..done + m], &right[done..done + m], &mut self.rs_l, &mut self.rs_r);
            let mut fed = 0;
            while fed < k {
                let s = self.stretcher.push(&self.rs_l[fed..k], &self.rs_r[fed..k]);
                fed += s;
                drain_into(&mut self.stretcher, &mut self.fifo, &mut self.fifo_len);
                if s == 0 {
                    break; // stretcher and FIFO both full: cannot happen with the sizes chosen in `new`
                }
            }
            drain_into(&mut self.stretcher, &mut self.fifo, &mut self.fifo_len);
            // Emit m frames from the FIFO (zeros if short, only at start-up).
            let avail = self.fifo_len.min(m);
            left[done..done + avail].copy_from_slice(&self.fifo[0][..avail]);
            right[done..done + avail].copy_from_slice(&self.fifo[1][..avail]);
            left[done + avail..done + m].fill(0.0);
            right[done + avail..done + m].fill(0.0);
            for ch in &mut self.fifo {
                ch.copy_within(avail..self.fifo_len, 0);
            }
            self.fifo_len -= avail;
            done += m;
        }
    }

    fn reset(&mut self) {
        self.resampler.reset();
        self.stretcher.reset();
        self.fifo_len = self.latency;
        for ch in &mut self.fifo {
            ch.fill(0.0);
        }
    }

    fn latency(&self) -> usize {
        self.latency
    }
}

/// Dominant frequency of a mono signal by autocorrelation (test helper and
/// a cheap pitch readout for the UI). Returns None for silence.
pub fn dominant_frequency(x: &[f32], rate: u32, min_hz: f32, max_hz: f32) -> Option<f32> {
    let n = x.len();
    let min_lag = (rate as f32 / max_hz) as usize;
    let max_lag = ((rate as f32 / min_hz) as usize).min(n / 2);
    if max_lag <= min_lag + 1 {
        return None;
    }
    let energy: f32 = x.iter().map(|v| v * v).sum();
    if energy < 1e-9 {
        return None;
    }
    let score = |lag: usize| -> f32 {
        let mut dot = 0.0;
        for i in 0..n - lag {
            dot += x[i] * x[i + lag];
        }
        dot / (n - lag) as f32
    };
    let scores: Vec<f32> = (min_lag..max_lag).map(score).collect();
    let best = scores.iter().cloned().fold(f32::MIN, f32::max);
    // The fundamental is the first lag that is a local maximum close to the
    // global best; later multiples of the period score about the same.
    let mut pick = None;
    for i in 1..scores.len().saturating_sub(1) {
        if scores[i] >= 0.9 * best && scores[i] >= scores[i - 1] && scores[i] >= scores[i + 1] {
            pick = Some(i);
            break;
        }
    }
    let i = pick?;
    // Parabolic refinement of the lag.
    let (a, b, c) = (scores[i - 1], scores[i], scores[i + 1]);
    let d = a - 2.0 * b + c;
    let off = if d.abs() > 1e-12 { (0.5 * (a - c) / d).clamp(-0.5, 0.5) } else { 0.0 };
    Some(rate as f32 / (min_lag as f32 + i as f32 + off))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Effect;

    fn sine(freq: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn stretch_changes_length_but_keeps_pitch_and_level() {
        let rate = 48_000;
        let x = sine(220.0, rate, rate as usize);
        for factor in [0.5, 0.8, 1.0, 1.5, 2.0] {
            let (l, r) = stretch(&x, &x, rate, factor);
            assert_eq!(l.len(), (48_000.0 * factor) as usize);
            assert_eq!(l, r);
            let mid = &l[l.len() / 4..l.len() * 3 / 4];
            let f = dominant_frequency(mid, rate, 100.0, 400.0).unwrap();
            assert!((f - 220.0).abs() < 3.0, "factor {factor}: {f} Hz");
            let level = rms(mid);
            assert!((level - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.08, "factor {factor}: rms {level}");
        }
    }

    #[test]
    fn stretch_of_unity_is_near_identity() {
        let rate = 48_000;
        let x: Vec<f32> = (0..20_000).map(|i| ((i * 7919) % 2000) as f32 / 1000.0 - 1.0).collect();
        let (l, _) = stretch(&x, &x, rate, 1.0);
        // At factor 1 the grains are the natural continuation, so WSOLA picks
        // offset 0 and Hann windows at 50 % overlap sum to one.
        for i in 2000..18_000 {
            assert!((l[i] - x[i]).abs() < 1e-3, "{i}: {} vs {}", l[i], x[i]);
        }
    }

    #[test]
    fn pitch_shift_moves_frequency_and_keeps_length() {
        let rate = 48_000;
        for (semis, want) in [(12.0f32, 440.0f32), (-12.0, 110.0), (7.0, 220.0 * 1.4983)] {
            let mut ps = PitchShifter::new(rate, semis, 512);
            let x = sine(220.0, rate, rate as usize * 2);
            let mut l = x.clone();
            let mut r = x.clone();
            for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                ps.process(a, b);
            }
            let mid = &l[rate as usize / 2..rate as usize * 3 / 2];
            let f = dominant_frequency(mid, rate, 50.0, 1000.0).unwrap();
            assert!((f - want).abs() < want * 0.02, "{semis} st: {f} Hz, want {want}");
            let level = rms(mid);
            assert!((level - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.1, "{semis} st: rms {level}");
        }
    }

    #[test]
    fn pitch_shift_zero_is_delay_only() {
        let rate = 48_000;
        let mut ps = PitchShifter::new(rate, 0.0, 256);
        let x = sine(300.0, rate, 24_000);
        let mut l = x.clone();
        let mut r = x.clone();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            ps.process(a, b);
        }
        let d = ps.latency();
        let mut err = 0.0f32;
        for i in d + 4000..24_000 {
            err = err.max((l[i] - x[i - d]).abs());
        }
        assert!(err < 0.05, "{err}");
    }
}
