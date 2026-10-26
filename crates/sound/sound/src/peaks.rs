// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Multi-resolution waveform peak caches for drawing clips at any zoom.
//!
//! Level 0 stores the min and max sample of every `base` frames (256 by
//! default: ~5 ms at 48 kHz, plenty for a timeline zoomed all the way in);
//! each further level halves the resolution. A query picks the coarsest
//! level that still has at least one bucket per pixel and merges buckets,
//! so drawing a two-hour clip across a 2000 px timeline touches a few
//! thousand entries, not hundreds of millions of samples.

/// Min and max of a span of samples.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Peak {
    /// Lowest sample value.
    pub min: f32,
    /// Highest sample value.
    pub max: f32,
}

impl Peak {
    const EMPTY: Peak = Peak { min: f32::MAX, max: f32::MIN };

    fn merge(self, other: Peak) -> Peak {
        Peak { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    fn normalise(self) -> Peak {
        if self.min > self.max { Peak::default() } else { self }
    }
}

/// One channel's pyramid.
#[derive(Clone, Debug, Default)]
struct Channel {
    levels: Vec<Vec<Peak>>,
    // Partial level-0 bucket being filled.
    pending: Peak,
    pending_n: usize,
}

/// Peak cache for one media item.
#[derive(Clone, Debug)]
pub struct PeakCache {
    rate: u32,
    base: usize,
    frames: u64,
    channels: Vec<Channel>,
}

impl PeakCache {
    /// An empty cache for `channels` channels at `rate`, `base` frames per
    /// finest bucket. Fill it with [`PeakCache::append`].
    pub fn new(channels: usize, rate: u32, base: usize) -> Self {
        let base = base.max(1);
        Self { rate, base, frames: 0, channels: (0..channels.max(1)).map(|_| Channel { levels: vec![Vec::new()], pending: Peak::EMPTY, pending_n: 0 }).collect() }
    }

    /// Builds a stereo cache from planar audio.
    pub fn build_stereo(left: &[f32], right: &[f32], rate: u32, base: usize) -> Self {
        let mut c = Self::new(2, rate, base);
        c.append(&[left, right]);
        c.finish();
        c
    }

    /// Builds a mono cache.
    pub fn build_mono(mono: &[f32], rate: u32, base: usize) -> Self {
        let mut c = Self::new(1, rate, base);
        c.append(&[mono]);
        c.finish();
        c
    }

    /// Appends planar samples (one slice per channel, equal lengths). Used
    /// while recording or decoding so the waveform appears as it grows.
    pub fn append(&mut self, planar: &[&[f32]]) {
        let n = planar.iter().map(|c| c.len()).min().unwrap_or(0);
        for (ch, data) in self.channels.iter_mut().zip(planar) {
            for &v in &data[..n] {
                ch.pending = ch.pending.merge(Peak { min: v, max: v });
                ch.pending_n += 1;
                if ch.pending_n == self.base {
                    let p = ch.pending;
                    ch.pending = Peak::EMPTY;
                    ch.pending_n = 0;
                    Self::push_level0(ch, p);
                }
            }
        }
        self.frames += n as u64;
    }

    fn push_level0(ch: &mut Channel, p: Peak) {
        ch.levels[0].push(p);
        // Propagate up: level k has one bucket per two of level k-1.
        let mut k = 0;
        while ch.levels[k].len() % 2 == 0 {
            let len = ch.levels[k].len();
            let merged = ch.levels[k][len - 2].merge(ch.levels[k][len - 1]);
            if ch.levels.len() == k + 1 {
                ch.levels.push(Vec::new());
            }
            ch.levels[k + 1].push(merged);
            k += 1;
            if ch.levels[k].len() < 2 {
                break;
            }
        }
    }

    /// Flushes a partial final bucket (call when the media is complete).
    pub fn finish(&mut self) {
        for ch in &mut self.channels {
            if ch.pending_n > 0 {
                let p = ch.pending;
                ch.pending = Peak::EMPTY;
                ch.pending_n = 0;
                Self::push_level0(ch, p);
            }
        }
    }

    /// Frames covered so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Channel count.
    pub fn channels(&self) -> usize {
        self.channels.len()
    }

    /// Sample rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Frames per finest bucket.
    pub fn base(&self) -> usize {
        self.base
    }

    /// Fills `out` with one peak per entry covering frames `start..end` of
    /// channel `ch`, evenly divided (one entry per pixel column). Regions past
    /// the data are silent.
    pub fn query(&self, ch: usize, start: u64, end: u64, out: &mut [Peak]) {
        out.fill(Peak::default());
        let Some(channel) = self.channels.get(ch) else { return };
        if out.is_empty() || end <= start {
            return;
        }
        let frames_per_px = (end - start) as f64 / out.len() as f64;
        // Coarsest level whose bucket is still at most one pixel wide.
        let mut level = 0usize;
        let mut bucket = self.base as f64;
        while level + 1 < channel.levels.len() && bucket * 2.0 <= frames_per_px {
            level += 1;
            bucket *= 2.0;
        }
        let data = &channel.levels[level];
        for (i, slot) in out.iter_mut().enumerate() {
            let f0 = start as f64 + i as f64 * frames_per_px;
            let f1 = f0 + frames_per_px;
            let b0 = (f0 / bucket).floor().max(0.0) as usize;
            let b1 = ((f1 / bucket).ceil() as usize).max(b0 + 1);
            let mut acc = Peak::EMPTY;
            for b in b0..b1.min(data.len()) {
                acc = acc.merge(data[b]);
            }
            if b1 > data.len() {
                // Past the end of this level: the finer levels may each hold
                // one unpaired trailing bucket, and level 0 a partial one.
                let mut fine_bucket = bucket;
                for j in (0..level).rev() {
                    fine_bucket /= 2.0;
                    let fine = &channel.levels[j];
                    if fine.len() % 2 == 1 {
                        let b = fine.len() - 1;
                        let (s0, s1) = (b as f64 * fine_bucket, (b + 1) as f64 * fine_bucket);
                        if s1 > f0 && s0 < f1 {
                            acc = acc.merge(fine[b]);
                        }
                    }
                }
                if channel.pending_n > 0 {
                    let s0 = channel.levels[0].len() as f64 * self.base as f64;
                    if s0 < f1 && s0 + channel.pending_n as f64 > f0 {
                        acc = acc.merge(channel.pending);
                    }
                }
            }
            *slot = acc.normalise();
        }
    }

    /// Overall peak of a channel.
    pub fn overall(&self, ch: usize) -> Peak {
        let Some(channel) = self.channels.get(ch) else { return Peak::default() };
        let top = channel.levels.last().map(|l| l.iter().fold(Peak::EMPTY, |a, b| a.merge(*b))).unwrap_or(Peak::EMPTY);
        let partial: Peak = channel.levels.iter().fold(Peak::EMPTY, |a, l| if l.len() % 2 == 1 { a.merge(*l.last().unwrap()) } else { a });
        top.merge(partial).merge(if channel.pending_n > 0 { channel.pending } else { Peak::EMPTY }).normalise()
    }

    /// Approximate memory use in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.channels.iter().map(|c| c.levels.iter().map(|l| l.len() * std::mem::size_of::<Peak>()).sum::<usize>()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brute(x: &[f32], start: usize, end: usize) -> Peak {
        x[start.min(x.len())..end.min(x.len())].iter().fold(Peak::EMPTY, |a, v| a.merge(Peak { min: *v, max: *v })).normalise()
    }

    #[test]
    fn queries_match_brute_force_at_every_zoom() {
        let n = 100_000;
        let x: Vec<f32> = (0..n).map(|i| ((i as f32 * 0.013).sin() * (1.0 + (i as f32 * 0.0001).cos())) * 0.5).collect();
        let c = PeakCache::build_mono(&x, 48_000, 256);
        assert_eq!(c.frames(), n as u64);
        for px in [1usize, 7, 100, 2000] {
            let mut out = vec![Peak::default(); px];
            c.query(0, 0, n as u64, &mut out);
            let per = n as f64 / px as f64;
            // The level the cache picks: the coarsest bucket within a pixel.
            let mut bucket = c.base();
            while (bucket * 2) as f64 <= per {
                bucket *= 2;
            }
            for (i, p) in out.iter().enumerate() {
                let (a, b) = ((i as f64 * per) as usize, ((i + 1) as f64 * per).ceil() as usize);
                let want = brute(&x, a, b);
                // Cache buckets may overhang the pixel by up to one bucket.
                let (a2, b2) = (a.saturating_sub(bucket), b + bucket);
                let loose = brute(&x, a2, b2);
                assert!(p.max >= want.max - 1e-6 && p.max <= loose.max + 1e-6, "px {px} col {i}: {p:?} vs {want:?}");
                assert!(p.min <= want.min + 1e-6 && p.min >= loose.min - 1e-6, "px {px} col {i} bucket {bucket}: {p:?} want {want:?} loose {loose:?} a={a} b={b}");
            }
        }
        let o = c.overall(0);
        let want = brute(&x, 0, n);
        assert!((o.max - want.max).abs() < 1e-6 && (o.min - want.min).abs() < 1e-6);
    }

    #[test]
    fn incremental_append_equals_one_shot_and_shows_partial_tail() {
        let x: Vec<f32> = (0..5000).map(|i| (i as f32 * 0.1).sin()).collect();
        let whole = PeakCache::build_mono(&x, 48_000, 64);
        let mut inc = PeakCache::new(1, 48_000, 64);
        for chunk in x.chunks(333) {
            inc.append(&[chunk]);
        }
        // Before finish, the last partial bucket is still visible in queries.
        let mut a = vec![Peak::default(); 50];
        let mut b = vec![Peak::default(); 50];
        inc.query(0, 0, 5000, &mut a);
        inc.finish();
        inc.query(0, 0, 5000, &mut b);
        assert_eq!(a, b);
        whole.query(0, 0, 5000, &mut a);
        assert_eq!(a, b);
        assert!(inc.memory_bytes() > 0);
        // Beyond the end is silent.
        let mut tail = vec![Peak::default(); 4];
        inc.query(0, 10_000, 20_000, &mut tail);
        assert!(tail.iter().all(|p| *p == Peak::default()));
    }
}
