// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sample-rate conversion with a windowed-sinc polyphase filter.
//!
//! The resampler is a streaming processor: push input blocks, get output
//! blocks, with the filter history kept across calls so block boundaries are
//! seamless. The conversion ratio can change between blocks (the pitch
//! shifter uses that), and a one-shot [`resample`] helper covers offline use.
//!
//! The filter is a Kaiser-windowed sinc with a cut-off just below the lower
//! of the two Nyquist frequencies, tabulated at 256 fractional phases and
//! linearly interpolated between them, which keeps the interpolation error
//! below the window's own stop-band leakage.

/// How much filtering to spend. Longer kernels mean a sharper transition
/// band and lower aliasing at a higher per-sample cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Quality {
    /// Linear interpolation; only for scrubbing previews and meters.
    Fast,
    /// 16-tap sinc (about −70 dB aliasing): live preview.
    Good,
    /// 32-tap sinc (about −90 dB): the default for monitoring.
    #[default]
    High,
    /// 64-tap sinc (about −110 dB): export.
    Best,
}

impl Quality {
    fn taps(self) -> usize {
        match self {
            Quality::Fast => 2,
            Quality::Good => 16,
            Quality::High => 32,
            Quality::Best => 64,
        }
    }

    fn kaiser_beta(self) -> f64 {
        match self {
            Quality::Fast => 0.0,
            Quality::Good => 7.0,
            Quality::High => 9.0,
            Quality::Best => 11.0,
        }
    }
}

const PHASES: usize = 256;

fn bessel_i0(x: f64) -> f64 {
    // Power series; converges quickly for the betas used here.
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k as f64 * k as f64);
        sum += term;
        if term < 1e-15 * sum {
            break;
        }
    }
    sum
}

/// Largest ratio the kernel lengthens for; above it aliasing rejection falls.
const MAX_SCALED_RATIO: f64 = 4.0;

/// Taps for a quality at a ratio: the base length stretched by the
/// downsampling factor so the kernel spans the same output-time width.
fn taps_for(quality: Quality, ratio: f64) -> usize {
    if quality == Quality::Fast {
        return 2;
    }
    let scale = ratio.clamp(1.0, MAX_SCALED_RATIO).ceil() as usize;
    quality.taps() * scale
}

/// A polyphase windowed-sinc kernel: `PHASES + 1` rows of `taps` coefficients
/// inside a table allocated for the largest supported length.
#[derive(Clone, Debug)]
struct Kernel {
    taps: usize,
    table: Vec<f32>,
}

impl Kernel {
    fn new(quality: Quality, ratio: f64) -> Self {
        let max_taps = taps_for(quality, MAX_SCALED_RATIO);
        let mut k = Self { taps: 2, table: vec![0f32; (PHASES + 1) * max_taps] };
        k.design(quality, ratio);
        k
    }

    /// Rewrites the table for `ratio` without allocating.
    fn design(&mut self, quality: Quality, ratio: f64) {
        let cutoff = 0.92 * (1.0 / ratio).min(1.0);
        let taps = taps_for(quality, ratio);
        self.taps = taps;
        let table = &mut self.table;
        let half = taps as f64 / 2.0;
        let beta = quality.kaiser_beta();
        let i0b = bessel_i0(beta);
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let row = &mut table[p * taps..(p + 1) * taps];
            let mut sum = 0.0;
            for (k, c) in row.iter_mut().enumerate() {
                // Tap k sits at input offset (k - half + 1) relative to the
                // integer position; the output point is `frac` past it.
                let x = k as f64 - half + 1.0 - frac;
                let v = if quality == Quality::Fast {
                    (1.0 - x.abs()).max(0.0)
                } else {
                    let s = if x.abs() < 1e-12 { cutoff } else { (std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x) };
                    let w = (x / half).clamp(-1.0, 1.0);
                    s * bessel_i0(beta * (1.0 - w * w).sqrt()) / i0b
                };
                *c = v as f32;
                sum += v;
            }
            // Normalise each phase to unity DC gain.
            if sum.abs() > 1e-9 {
                for c in row.iter_mut() {
                    *c = (*c as f64 / sum) as f32;
                }
            }
        }
    }

    /// Interpolates `hist[idx - taps/2 + 1 ..= idx + taps/2]` at fraction `frac`.
    #[inline]
    fn at(&self, hist: &[f32], idx: usize, frac: f64) -> f32 {
        let pf = frac * PHASES as f64;
        let p = pf as usize;
        let mix = (pf - p as f64) as f32;
        let a = &self.table[p * self.taps..(p + 1) * self.taps];
        let b = &self.table[(p + 1) * self.taps..(p + 2) * self.taps];
        let start = idx + 1 - self.taps / 2;
        let mut acc = 0.0;
        for k in 0..self.taps {
            let c = a[k] + (b[k] - a[k]) * mix;
            acc += hist[start + k] * c;
        }
        acc
    }
}

/// Streaming stereo resampler.
pub struct Resampler {
    kernel: Kernel,
    ratio: f64,
    /// Fractional read position within `buf` (in input frames).
    pos: f64,
    /// Input history plus the latest block, per channel.
    buf: [Vec<f32>; 2],
    /// Valid frames in `buf`.
    len: usize,
    quality: Quality,
    max_input: usize,
}

impl Resampler {
    /// A resampler from `from` Hz to `to` Hz accepting up to `max_block`
    /// input frames per call. Output blocks need `max_block * to / from + 2`
    /// frames of room; see [`Resampler::max_output`].
    pub fn new(from: u32, to: u32, quality: Quality, max_block: usize) -> Self {
        let ratio = from as f64 / to.max(1) as f64;
        Self::with_ratio(ratio, quality, max_block)
    }

    /// A resampler with an explicit step `ratio` = input frames consumed per
    /// output frame (2.0 halves the length, 0.5 doubles it).
    pub fn with_ratio(ratio: f64, quality: Quality, max_block: usize) -> Self {
        let ratio = ratio.clamp(1.0 / 64.0, 64.0);
        let kernel = Kernel::new(quality, ratio);
        let max_taps = taps_for(quality, MAX_SCALED_RATIO);
        let cap = max_block.max(1) + max_taps + 2;
        let mut r = Self { kernel, ratio, pos: 0.0, buf: [vec![0.0; cap], vec![0.0; cap]], len: 0, quality, max_input: max_block.max(1) };
        r.reset();
        r
    }

    /// Current step ratio (input frames per output frame).
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Changes the ratio for following blocks. When the ratio moves above
    /// 1.0 or changes the kernel length, the table is rewritten in place
    /// (allocation-free). The kernel's maximum length is fixed at
    /// construction, so the history buffer always fits.
    pub fn set_ratio(&mut self, ratio: f64) {
        let ratio = ratio.clamp(1.0 / 64.0, 64.0);
        if (ratio - self.ratio).abs() < 1e-12 {
            return;
        }
        let redesign = ratio > 1.0 || self.ratio > 1.0 || taps_for(self.quality, ratio) != self.kernel.taps;
        let old_half = self.kernel.taps / 2;
        self.ratio = ratio;
        if redesign {
            self.kernel.design(self.quality, ratio);
            // Keep the output aligned: the read position is anchored to the
            // kernel's centre, which moved with the length.
            let new_half = self.kernel.taps / 2;
            self.pos += new_half as f64 - old_half as f64;
            if new_half > old_half {
                // Need more history on the left: shift the buffer right.
                let extra = new_half - old_half;
                for ch in &mut self.buf {
                    ch.copy_within(0..self.len, extra);
                    ch[..extra].fill(0.0);
                }
                self.len += extra;
            }
        }
    }

    /// Input frames the filter must see past an output point before it can
    /// produce it (half the kernel). Output frame `j` corresponds exactly to
    /// input position `j * ratio`; this is look-ahead, not a delay.
    pub fn latency(&self) -> usize {
        self.kernel.taps / 2
    }

    /// The largest number of output frames one call to [`Resampler::process`]
    /// can produce for the given input length and current ratio.
    pub fn max_output(&self, input_frames: usize) -> usize {
        (input_frames as f64 / self.ratio).ceil() as usize + 2
    }

    /// Input frames that can be pushed right now without overflowing the
    /// history buffer (it shrinks when output was not drained).
    pub fn space(&self) -> usize {
        (self.buf[0].len() - self.len).min(self.max_input)
    }

    /// Resamples one block. Up to [`Resampler::space`] input frames are
    /// taken from `left`/`right` (extra frames are ignored; check `space`
    /// when that matters); output goes to `out_l`/`out_r`, returning the
    /// frames produced. When the output slices fill up, the unconsumed input
    /// stays buffered and comes out on the next call, so nothing is lost.
    pub fn process(&mut self, left: &[f32], right: &[f32], out_l: &mut [f32], out_r: &mut [f32]) -> usize {
        let n = left.len().min(right.len()).min(self.space());
        let cap = self.buf[0].len();
        debug_assert!(self.len + n <= cap);
        self.buf[0][self.len..self.len + n].copy_from_slice(&left[..n]);
        self.buf[1][self.len..self.len + n].copy_from_slice(&right[..n]);
        self.len += n;
        let half = self.kernel.taps / 2;
        let last_ok = self.len.saturating_sub(half); // need idx + half < len
        let max_out = out_l.len().min(out_r.len());
        let mut produced = 0;
        while produced < max_out {
            let idx = self.pos.floor() as usize;
            if idx >= last_ok {
                break;
            }
            let frac = self.pos - idx as f64;
            out_l[produced] = self.kernel.at(&self.buf[0], idx, frac);
            out_r[produced] = self.kernel.at(&self.buf[1], idx, frac);
            produced += 1;
            self.pos += self.ratio;
        }
        // Keep the history the next block needs: everything from idx - half.
        let idx = self.pos.floor() as usize;
        let keep_from = idx.saturating_sub(half).min(self.len);
        if keep_from > 0 {
            for ch in &mut self.buf {
                ch.copy_within(keep_from..self.len, 0);
            }
            self.len -= keep_from;
            self.pos -= keep_from as f64;
        }
        produced
    }

    /// Clears the history (on seek). The next input frame pushed is input
    /// position 0, and the first output produced corresponds to it.
    pub fn reset(&mut self) {
        let half = self.kernel.taps / 2;
        for ch in &mut self.buf {
            ch.fill(0.0);
        }
        // `half` zeros of pre-history so the kernel's left half has data;
        // the read position sits on the first real input frame.
        self.len = half;
        self.pos = half as f64;
    }

    /// Flushes the tail after the last input: pads with silence so the final
    /// `latency()` frames come out. Returns frames produced.
    pub fn flush(&mut self, out_l: &mut [f32], out_r: &mut [f32]) -> usize {
        let zeros = vec![0.0; self.kernel.taps.min(self.max_input)]; // not on the real-time path
        let mut total = 0;
        let mut fed = 0;
        while fed < self.kernel.taps {
            let k = self.process(&zeros, &zeros, &mut out_l[total..], &mut out_r[total..]);
            total += k;
            fed += zeros.len();
        }
        total
    }
}

/// One-shot resampling of planar stereo (`[left, right]`) from `from` to `to`
/// Hz. Allocates; for offline work (import conforming, export).
pub fn resample(left: &[f32], right: &[f32], from: u32, to: u32, quality: Quality) -> (Vec<f32>, Vec<f32>) {
    if from == to {
        return (left.to_vec(), right.to_vec());
    }
    let n = left.len().min(right.len());
    let block = 1024;
    let mut rs = Resampler::new(from, to, quality, block);
    let expect = (n as f64 * to as f64 / from as f64).round() as usize;
    let mut out_l = Vec::with_capacity(expect + 64);
    let mut out_r = Vec::with_capacity(expect + 64);
    let mut tl = vec![0.0; rs.max_output(block)];
    let mut tr = vec![0.0; rs.max_output(block)];
    let mut i = 0;
    while i < n {
        let m = (n - i).min(block);
        let k = rs.process(&left[i..i + m], &right[i..i + m], &mut tl, &mut tr);
        out_l.extend_from_slice(&tl[..k]);
        out_r.extend_from_slice(&tr[..k]);
        i += m;
    }
    let k = rs.flush(&mut tl, &mut tr);
    out_l.extend_from_slice(&tl[..k]);
    out_r.extend_from_slice(&tr[..k]);
    // Output frame j is input position j * from / to, so only the length needs trimming.
    out_l.resize(expect, 0.0);
    out_r.resize(expect, 0.0);
    (out_l, out_r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (std::f64::consts::TAU * freq * i as f64 / rate as f64).sin() as f32).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn kernel_rows_have_unity_dc_gain() {
        for q in [Quality::Fast, Quality::Good, Quality::High, Quality::Best] {
            let k = Kernel::new(q, 1.3);
            for p in 0..=PHASES {
                let s: f32 = k.table[p * k.taps..(p + 1) * k.taps].iter().sum();
                assert!((s - 1.0).abs() < 1e-5, "{q:?} phase {p}: {s}");
            }
        }
    }

    #[test]
    fn upsample_preserves_a_sine() {
        let n = 44_100;
        let x = sine(1000.0, 44_100, n);
        let (l, _) = resample(&x, &x, 44_100, 48_000, Quality::High);
        assert_eq!(l.len(), 48_000);
        // Compare the middle against the analytic sine at the new rate.
        let mut err = 0.0f32;
        for i in 1000..47_000 {
            let want = (std::f64::consts::TAU * 1000.0 * i as f64 / 48_000.0).sin() as f32;
            err = err.max((l[i] - want).abs());
        }
        assert!(err < 2e-3, "max error {err}");
    }

    #[test]
    fn downsample_rejects_aliasing_components() {
        // A 32 kHz tone at 96 kHz must vanish when going to 44.1 kHz (it
        // would alias to 12.1 kHz otherwise).
        let x = sine(32_000.0, 96_000, 96_000);
        for (q, limit_db) in [(Quality::Good, -45.0), (Quality::High, -65.0), (Quality::Best, -80.0)] {
            let (l, _) = resample(&x, &x, 96_000, 44_100, q);
            let level = crate::gain_to_db(rms(&l[2000..40_000]) / rms(&x[2000..40_000]));
            assert!(level < limit_db, "{q:?}: {level} dB");
        }
        // And a 1 kHz tone passes unchanged.
        let x = sine(1000.0, 96_000, 96_000);
        let (l, _) = resample(&x, &x, 96_000, 44_100, Quality::High);
        let level = crate::gain_to_db(rms(&l[2000..40_000]) / rms(&x[2000..40_000]));
        assert!(level.abs() < 0.05, "{level} dB");
    }

    #[test]
    fn streaming_blocks_match_one_shot() {
        let x = sine(440.0, 48_000, 20_000);
        let (whole, _) = resample(&x, &x, 48_000, 44_100, Quality::High);
        let mut rs = Resampler::new(48_000, 44_100, Quality::High, 256);
        let mut got = Vec::new();
        let (mut tl, mut tr) = (vec![0.0; rs.max_output(256)], vec![0.0; rs.max_output(256)]);
        let mut i = 0;
        // Irregular block sizes to exercise the history handling.
        for size in [256usize, 100, 7, 256, 1, 200].iter().cycle() {
            if i >= x.len() {
                break;
            }
            let m = (*size).min(x.len() - i);
            let k = rs.process(&x[i..i + m], &x[i..i + m], &mut tl, &mut tr);
            got.extend_from_slice(&tl[..k]);
            i += m;
        }
        assert!(got.len() + 40 >= whole.len(), "{} vs {}", got.len(), whole.len());
        for (i, (a, b)) in whole.iter().zip(got.iter()).enumerate() {
            assert!((a - b).abs() < 1e-5, "frame {i}: {a} vs {b}");
        }
    }

    #[test]
    fn same_rate_is_identity_and_ratio_changes_work() {
        let x = sine(100.0, 48_000, 1000);
        let (l, r) = resample(&x, &x, 48_000, 48_000, Quality::Best);
        assert_eq!(l, x);
        assert_eq!(r, x);
        let mut rs = Resampler::with_ratio(2.0, Quality::Good, 512);
        let (mut tl, mut tr) = (vec![0.0; 600], vec![0.0; 600]);
        let k = rs.process(&x[..512], &x[..512], &mut tl, &mut tr);
        assert!((230..=258).contains(&k), "{k}");
        rs.set_ratio(0.5);
        let k = rs.process(&x[..512], &x[..512], &mut tl, &mut tr);
        assert!(k > 500 && k <= 600, "{k}");
        for v in &tl[..k] {
            assert!(v.is_finite() && v.abs() <= 1.01);
        }
    }
}
