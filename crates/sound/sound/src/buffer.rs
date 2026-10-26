// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Block buffers, gain helpers and the [`Effect`] trait shared by every
//! processor in the crate.

/// Decibels to linear gain. `f32::NEG_INFINITY` (and anything below −200 dB)
/// is silence.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -200.0 { 0.0 } else { 10f32.powf(db * 0.05) }
}

/// Linear gain to decibels; zero or negative gain is `NEG_INFINITY`.
#[inline]
pub fn gain_to_db(g: f32) -> f32 {
    if g <= 0.0 { f32::NEG_INFINITY } else { 20.0 * g.log10() }
}

/// A stereo block in planar layout with capacity fixed at construction, so
/// filling or clearing it never allocates.
#[derive(Clone, Debug)]
pub struct StereoBuf {
    left: Vec<f32>,
    right: Vec<f32>,
    frames: usize,
}

impl StereoBuf {
    /// A silent buffer holding `capacity` frames, reporting `capacity` frames long.
    pub fn new(capacity: usize) -> Self {
        Self { left: vec![0.0; capacity], right: vec![0.0; capacity], frames: capacity }
    }

    /// The allocated capacity in frames.
    pub fn capacity(&self) -> usize {
        self.left.len()
    }

    /// The current block length in frames.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Sets the block length; panics if above capacity (a programming error).
    pub fn set_frames(&mut self, frames: usize) {
        assert!(frames <= self.left.len(), "block of {frames} frames exceeds capacity {}", self.left.len());
        self.frames = frames;
    }

    /// Zeroes the active block.
    pub fn clear(&mut self) {
        let n = self.frames;
        self.left[..n].fill(0.0);
        self.right[..n].fill(0.0);
    }

    /// Left channel of the active block.
    pub fn left(&self) -> &[f32] {
        &self.left[..self.frames]
    }

    /// Right channel of the active block.
    pub fn right(&self) -> &[f32] {
        &self.right[..self.frames]
    }

    /// Both channels of the active block, mutably.
    pub fn channels_mut(&mut self) -> (&mut [f32], &mut [f32]) {
        let n = self.frames;
        (&mut self.left[..n], &mut self.right[..n])
    }

    /// Adds `other` (same length) into this block.
    pub fn add(&mut self, other: &StereoBuf) {
        let n = self.frames.min(other.frames);
        for i in 0..n {
            self.left[i] += other.left[i];
            self.right[i] += other.right[i];
        }
    }

    /// Adds `other` scaled by `gain`.
    pub fn add_scaled(&mut self, other: &StereoBuf, gain: f32) {
        let n = self.frames.min(other.frames);
        for i in 0..n {
            self.left[i] += other.left[i] * gain;
            self.right[i] += other.right[i] * gain;
        }
    }

    /// Multiplies both channels by a per-sample gain ramp.
    pub fn apply_gain(&mut self, gains: &[f32]) {
        let n = self.frames.min(gains.len());
        for i in 0..n {
            self.left[i] *= gains[i];
            self.right[i] *= gains[i];
        }
    }

    /// Copies `other` into this block (lengths follow `other`).
    pub fn copy_from(&mut self, other: &StereoBuf) {
        let n = other.frames.min(self.left.len());
        self.frames = n;
        self.left[..n].copy_from_slice(&other.left[..n]);
        self.right[..n].copy_from_slice(&other.right[..n]);
    }

    /// Writes the active block interleaved (L R L R …) into `out`, returning
    /// the number of samples written.
    pub fn write_interleaved(&self, out: &mut [f32]) -> usize {
        let n = self.frames.min(out.len() / 2);
        for i in 0..n {
            out[2 * i] = self.left[i];
            out[2 * i + 1] = self.right[i];
        }
        n * 2
    }

    /// Loads interleaved stereo into the block, setting its length.
    pub fn read_interleaved(&mut self, input: &[f32]) {
        let n = (input.len() / 2).min(self.left.len());
        self.frames = n;
        for i in 0..n {
            self.left[i] = input[2 * i];
            self.right[i] = input[2 * i + 1];
        }
    }

    /// Peak absolute sample value over both channels.
    pub fn peak(&self) -> f32 {
        let n = self.frames;
        self.left[..n].iter().chain(&self.right[..n]).fold(0.0f32, |m, v| m.max(v.abs()))
    }
}

/// A stereo in-place block processor. Implementations never allocate in
/// [`Effect::process`]; all state is sized by the constructor for the maximum
/// block length it was given.
pub trait Effect: Send {
    /// Processes one block in place. Both slices have the same length, which
    /// never exceeds the maximum the processor was built for.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);

    /// Forgets all signal history (delay lines, envelopes). Parameters stay.
    fn reset(&mut self);

    /// Frames of delay the processor adds, for latency compensation.
    fn latency(&self) -> usize {
        0
    }

    /// Frames after the input stops that the output is still non-silent
    /// (reverb and delay tails). `usize::MAX` means "until reset".
    fn tail(&self) -> usize {
        0
    }
}

/// Fills `out` with a cosine-shaped equal-power stereo pan law:
/// `pan` −1 = hard left, 0 = centre (−3 dB each side), +1 = hard right.
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * std::f32::consts::PI;
    (p.cos(), p.sin())
}

/// "Fill channel": copies one channel onto the other (`left_to_right` or the
/// reverse), for one-sided recordings.
pub fn fill_channel(left: &mut [f32], right: &mut [f32], left_to_right: bool) {
    if left_to_right { right.copy_from_slice(left) } else { left.copy_from_slice(right) }
}

/// Reverses a stereo block (used for reversed clips once decoded).
pub fn reverse(left: &mut [f32], right: &mut [f32]) {
    left.reverse();
    right.reverse();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_conversions() {
        assert!((db_to_gain(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
        assert!((gain_to_db(2.0) - 6.0206).abs() < 1e-3);
        assert_eq!(db_to_gain(f32::NEG_INFINITY), 0.0);
        assert_eq!(gain_to_db(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn pan_law_is_equal_power() {
        for pan in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            let (l, r) = pan_gains(pan);
            assert!((l * l + r * r - 1.0).abs() < 1e-5);
        }
        let (l, r) = pan_gains(0.0);
        assert!((gain_to_db(l) + 3.01).abs() < 0.02 && (l - r).abs() < 1e-6);
        assert_eq!(pan_gains(-1.0).1.abs() < 1e-6, true);
    }

    #[test]
    fn interleave_round_trip() {
        let mut b = StereoBuf::new(4);
        b.read_interleaved(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(b.frames(), 3);
        assert_eq!(b.left(), &[1.0, 3.0, 5.0]);
        let mut out = [0.0; 6];
        assert_eq!(b.write_interleaved(&mut out), 6);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(b.peak(), 6.0);
    }
}
