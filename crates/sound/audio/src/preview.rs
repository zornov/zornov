// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Click-to-preview playback for the Audio material panel: one item at a
//! time on its own output queue, independent of timeline playback.

use crate::{AudioOut, RATE};

/// Linear-interpolation resampling of interleaved stereo from `from` Hz to `to` Hz.
pub fn resample_stereo(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 {
        return input.to_vec();
    }
    let frames = input.len() / 2;
    let out_frames = (frames as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    let at = |i: usize, c: usize| input.get(i * 2 + c).copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(out_frames * 2);
    for k in 0..out_frames {
        let p = k as f64 * step;
        let i = p.floor() as usize;
        let f = (p - i as f64) as f32;
        for c in 0..2 {
            out.push(at(i, c) * (1.0 - f) + at(i + 1, c) * f);
        }
    }
    out
}

/// The panel's preview player. The output queue opens on first use.
#[derive(Default)]
pub struct PanelPreview {
    out: Option<AudioOut>,
    key: Option<String>,
    total: usize,
    start_played: u64,
    error: Option<String>,
}

impl PanelPreview {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plays interleaved stereo at [`RATE`] for item `key`, replacing anything playing.
    pub fn play(&mut self, key: &str, stereo: &[f32]) {
        if self.out.is_none() {
            match AudioOut::open() {
                Ok(o) => self.out = Some(o),
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        }
        let out = self.out.as_ref().unwrap();
        out.clear();
        out.push(stereo);
        self.key = Some(key.to_string());
        self.total = stereo.len() / 2;
        self.start_played = out.played_frames();
    }

    pub fn stop(&mut self) {
        if let Some(o) = &self.out {
            o.clear();
        }
        self.key = None;
    }

    /// Key of the item playing now (None once it has finished).
    pub fn playing(&mut self) -> Option<&str> {
        let done = self.out.as_ref().is_none_or(|o| o.queued_frames() == 0);
        if done {
            self.key = None;
        }
        self.key.as_deref()
    }

    /// Fraction played of the current item, 0..1.
    pub fn progress(&self) -> f32 {
        match (&self.out, self.key.is_some()) {
            (Some(o), true) if self.total > 0 => ((self.total - o.queued_frames().min(self.total)) as f32 / self.total as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    /// Why the output could not open, if it could not.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn rate(&self) -> u32 {
        RATE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_doubles_and_halves() {
        let x: Vec<f32> = (0..100).flat_map(|i| [i as f32, -(i as f32)]).collect();
        let up = resample_stereo(&x, 24_000, 48_000);
        assert_eq!(up.len(), 400);
        assert_eq!(&up[..6], &[0.0, -0.0, 0.5, -0.5, 1.0, -1.0]);
        let down = resample_stereo(&x, 48_000, 24_000);
        assert_eq!(down.len(), 100);
        assert_eq!(down[2], 2.0);
        assert_eq!(resample_stereo(&x, 48_000, 48_000), x);
    }

    #[test]
    fn preview_plays_and_stops() {
        let mut p = PanelPreview::new();
        p.play("a", &[0.0; 9600]);
        if p.error().is_some() {
            return; // no output device here
        }
        assert_eq!(p.playing(), Some("a"));
        p.stop();
        assert_eq!(p.playing(), None);
    }
}
