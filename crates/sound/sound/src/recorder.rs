// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Voice-over recorder logic: count-in, input metering, take assembly and
//! a growing peak cache for the lane being recorded, independent of the
//! microphone API so it can be tested with synthetic input.
//!
//! The app owns the device (`studio_audio::AudioIn` on macOS), feeds the
//! samples it receives to [`VoiceOverRecorder::feed`], and pulls count-in
//! clicks for the monitor output from [`VoiceOverRecorder::render_clicks`].

use crate::Micros;
use crate::buffer::gain_to_db;
use crate::peaks::PeakCache;

/// Recorder settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecorderParams {
    /// Beats of count-in before recording starts (0, 3 or 4 in the UI).
    pub count_in_beats: u32,
    /// Count-in tempo in BPM.
    pub count_in_bpm: f32,
    /// Timeline position where the take starts.
    pub start: Micros,
    /// Input gain applied to the samples (0..2, shown as 0–200 %).
    pub gain: f32,
    /// Trim silence under this level (dBFS) from the start and end of the
    /// take; `None` keeps everything.
    pub trim_silence_below_db: Option<f32>,
}

impl Default for RecorderParams {
    fn default() -> Self {
        Self { count_in_beats: 3, count_in_bpm: 120.0, start: 0, gain: 1.0, trim_silence_below_db: Some(-50.0) }
    }
}

/// Where the recorder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecorderState {
    /// Armed but not started: the meter runs, nothing is kept.
    Idle,
    /// Counting in; `beats_left` beats remain (the UI shows 3, 2, 1).
    CountIn {
        /// Beats still to go, including the current one.
        beats_left: u32,
    },
    /// Recording.
    Recording,
    /// Stopped with a take ready to collect.
    Finished,
}

/// Peak and RMS input meter with a peak-hold indicator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputMeter {
    /// Latest block peak in dBFS.
    pub peak_db: f32,
    /// Latest block RMS in dBFS.
    pub rms_db: f32,
    /// Highest peak in the last ~1.5 s, in dBFS.
    pub hold_db: f32,
    /// True if any sample hit full scale.
    pub clipped: bool,
    hold_left: u32,
}

impl Default for InputMeter {
    fn default() -> Self {
        Self { peak_db: f32::NEG_INFINITY, rms_db: f32::NEG_INFINITY, hold_db: f32::NEG_INFINITY, clipped: false, hold_left: 0 }
    }
}

/// A finished recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    /// Mono samples at the recorder's rate.
    pub samples: Vec<f32>,
    /// Sample rate.
    pub rate: u32,
    /// Timeline position of the first sample (after trimming).
    pub start: Micros,
    /// Duration from the samples.
    pub duration: Micros,
    /// Highest absolute sample.
    pub peak: f32,
}

/// The recorder state machine.
pub struct VoiceOverRecorder {
    rate: u32,
    params: RecorderParams,
    state: RecorderState,
    // Count-in clock in frames since the count-in began.
    count_frames: u64,
    beat_frames: u64,
    samples: Vec<f32>,
    meter: InputMeter,
    peaks: PeakCache,
    // Click synthesis.
    click_pos: u64,
    monitor_muted: bool,
}

impl VoiceOverRecorder {
    /// A recorder for input at `rate`.
    pub fn new(rate: u32, params: RecorderParams) -> Self {
        let beat_frames = (60.0 / params.count_in_bpm.max(20.0) * rate as f32) as u64;
        Self { rate, params, state: RecorderState::Idle, count_frames: 0, beat_frames, samples: Vec::new(), meter: InputMeter::default(), peaks: PeakCache::new(1, rate, 256), click_pos: 0, monitor_muted: true }
    }

    /// Settings.
    pub fn params(&self) -> &RecorderParams {
        &self.params
    }

    /// Current state.
    pub fn state(&self) -> RecorderState {
        self.state
    }

    /// The input meter (updates on every [`feed`](Self::feed), in any state).
    pub fn meter(&self) -> &InputMeter {
        &self.meter
    }

    /// Peak cache of the take so far, for drawing the growing clip.
    pub fn peaks(&self) -> &PeakCache {
        &self.peaks
    }

    /// Frames recorded so far.
    pub fn recorded_frames(&self) -> usize {
        self.samples.len()
    }

    /// Whether the timeline's other audio should be muted in the monitor
    /// while recording (the "echo reduction" option: with speakers instead
    /// of headphones, playback would leak back into the microphone).
    pub fn monitor_muted(&self) -> bool {
        self.monitor_muted
    }

    /// Sets the monitor-mute option.
    pub fn set_monitor_muted(&mut self, muted: bool) {
        self.monitor_muted = muted;
    }

    /// Starts the count-in (or recording at once when there is none).
    pub fn start(&mut self) {
        self.samples.clear();
        self.peaks = PeakCache::new(1, self.rate, 256);
        self.count_frames = 0;
        self.click_pos = 0;
        self.meter.clipped = false;
        self.state = if self.params.count_in_beats == 0 { RecorderState::Recording } else { RecorderState::CountIn { beats_left: self.params.count_in_beats } };
    }

    /// Feeds microphone samples (mono at the recorder's rate). The meter
    /// always updates; samples are kept only while recording.
    pub fn feed(&mut self, input: &[f32]) {
        let g = self.params.gain.clamp(0.0, 2.0);
        // Meter on the gained signal, in place-free fashion.
        let mut peak = 0.0f32;
        let mut sum = 0.0f64;
        for &x in input {
            let v = (x * g).clamp(-1.0, 1.0);
            peak = peak.max(v.abs());
            sum += (v * v) as f64;
        }
        if !input.is_empty() {
            self.meter.update_from(peak, (sum / input.len() as f64).sqrt() as f32, input.len(), self.rate);
        }
        let mut offset = 0;
        if let RecorderState::CountIn { .. } = self.state {
            let total = self.beat_frames * self.params.count_in_beats as u64;
            let remaining = total.saturating_sub(self.count_frames);
            let advance = (input.len() as u64).min(remaining);
            self.count_frames += advance;
            offset = advance as usize;
            if self.count_frames >= total {
                self.state = RecorderState::Recording;
            } else {
                let beats_done = (self.count_frames / self.beat_frames) as u32;
                self.state = RecorderState::CountIn { beats_left: self.params.count_in_beats - beats_done };
            }
        }
        if self.state == RecorderState::Recording && offset < input.len() {
            let start = self.samples.len();
            self.samples.extend(input[offset..].iter().map(|x| (x * g).clamp(-1.0, 1.0)));
            self.peaks.append(&[&self.samples[start..]]);
        }
    }

    /// Renders the count-in click track for the monitor output: `out` is
    /// interleaved stereo at the recorder's rate, filled in sync with the
    /// frames fed so far. Silent outside the count-in. Each beat is a short
    /// 1 kHz tick; the last beat is higher so the ear knows to start.
    pub fn render_clicks(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        let total = self.beat_frames * self.params.count_in_beats as u64;
        let tick_len = (0.03 * self.rate as f32) as u64;
        for i in 0..frames {
            let t = self.click_pos + i as u64;
            let v = if t < total && self.beat_frames > 0 {
                let beat = t / self.beat_frames;
                let in_beat = t % self.beat_frames;
                if in_beat < tick_len {
                    let f = if beat + 1 == self.params.count_in_beats as u64 { 1500.0 } else { 1000.0 };
                    let env = 1.0 - in_beat as f32 / tick_len as f32;
                    0.4 * env * (std::f32::consts::TAU * f * in_beat as f32 / self.rate as f32).sin()
                } else {
                    0.0
                }
            } else {
                0.0
            };
            out[2 * i] = v;
            out[2 * i + 1] = v;
        }
        self.click_pos += frames as u64;
    }

    /// Stops and returns the take (`None` when nothing was recorded).
    pub fn stop(&mut self) -> Option<Take> {
        self.state = RecorderState::Finished;
        self.peaks.finish();
        if self.samples.is_empty() {
            return None;
        }
        let mut samples = std::mem::take(&mut self.samples);
        let mut start = self.params.start;
        if let Some(db) = self.params.trim_silence_below_db {
            let thr = crate::db_to_gain(db);
            // Trim in 10 ms blocks so a single stray sample does not count.
            let blk = (self.rate as usize / 100).max(1);
            let loud = |b: &[f32]| b.iter().any(|v| v.abs() > thr);
            let first = samples.chunks(blk).position(loud).map(|i| i * blk);
            let last = samples.chunks(blk).rposition(loud).map(|i| ((i + 1) * blk).min(samples.len()));
            if let (Some(a), Some(b)) = (first, last) {
                // Keep 50 ms of handle either side so breaths are not clipped.
                let handle = self.rate as usize / 20;
                let a = a.saturating_sub(handle);
                let b = (b + handle).min(samples.len());
                samples.drain(b..);
                samples.drain(..a);
                start += crate::frames_to_micros(a as i64, self.rate);
            }
        }
        let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let duration = crate::frames_to_micros(samples.len() as i64, self.rate);
        Some(Take { samples, rate: self.rate, start, duration, peak })
    }

    /// Back to idle, discarding any take.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.state = RecorderState::Idle;
        self.count_frames = 0;
        self.click_pos = 0;
    }
}

impl InputMeter {
    fn update_from(&mut self, peak: f32, rms: f32, block_len: usize, rate: u32) {
        self.peak_db = gain_to_db(peak);
        self.rms_db = gain_to_db(rms);
        if peak >= 0.999 {
            self.clipped = true;
        }
        let blocks_per_1_5s = (1.5 * rate as f32 / block_len.max(1) as f32) as u32;
        if self.peak_db >= self.hold_db || self.hold_left == 0 {
            self.hold_db = self.peak_db;
            self.hold_left = blocks_per_1_5s;
        } else {
            self.hold_left -= 1;
        }
    }
}

/// Reads the microphone through `studio_audio::AudioIn` and feeds a
/// recorder. The device is opened by the app; this just moves samples.
pub struct DeviceFeed<'a> {
    input: &'a studio_audio::AudioIn,
}

impl<'a> DeviceFeed<'a> {
    /// Wraps an open input.
    pub fn new(input: &'a studio_audio::AudioIn) -> Self {
        input.set_recording(true);
        Self { input }
    }

    /// Moves everything the device captured since the last call into the
    /// recorder.
    pub fn pump(&self, recorder: &mut VoiceOverRecorder) {
        let samples = self.input.take_samples();
        recorder.feed(&samples);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_in_then_record_then_trim() {
        let rate = 48_000;
        let mut r = VoiceOverRecorder::new(rate, RecorderParams { count_in_beats: 3, count_in_bpm: 120.0, start: 5_000_000, gain: 1.0, trim_silence_below_db: Some(-50.0) });
        assert_eq!(r.state(), RecorderState::Idle);
        r.start();
        assert_eq!(r.state(), RecorderState::CountIn { beats_left: 3 });
        // Count-in is 1.5 s = 72 000 frames; feed 0.6 s of silence twice.
        let quiet = vec![0.0f32; 28_800];
        r.feed(&quiet);
        assert_eq!(r.state(), RecorderState::CountIn { beats_left: 2 });
        r.feed(&quiet);
        assert_eq!(r.state(), RecorderState::CountIn { beats_left: 1 });
        // Clicks exist during the count-in.
        let mut clicks = vec![0.0f32; 4800 * 2];
        r.render_clicks(&mut clicks);
        assert!(clicks.iter().any(|v| v.abs() > 0.1));
        // Cross the boundary: 0.6 s more → 0.3 s into recording, silence.
        r.feed(&quiet);
        assert_eq!(r.state(), RecorderState::Recording);
        assert_eq!(r.recorded_frames(), 14_400);
        // Then 1 s of tone, then 0.5 s silence.
        let tone: Vec<f32> = (0..rate as usize).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin()).collect();
        r.feed(&tone);
        assert!((r.meter().peak_db + 6.02).abs() < 0.1, "{}", r.meter().peak_db);
        assert!((r.meter().rms_db + 9.03).abs() < 0.1, "{}", r.meter().rms_db);
        assert!(!r.meter().clipped);
        r.feed(&vec![0.0; 24_000]);
        assert!(r.peaks().frames() > 0);
        let take = r.stop().unwrap();
        assert_eq!(r.state(), RecorderState::Finished);
        // Trimmed to the tone plus 50 ms handles: start moved by 0.3 s − 50 ms.
        assert_eq!(take.start, 5_000_000 + 300_000 - 50_000);
        assert!((take.duration - 1_100_000).abs() < 11_000, "{}", take.duration);
        assert!((take.peak - 0.5).abs() < 1e-3);
        assert_eq!(take.rate, rate);
    }

    #[test]
    fn no_count_in_and_empty_take() {
        let mut r = VoiceOverRecorder::new(48_000, RecorderParams { count_in_beats: 0, ..Default::default() });
        r.start();
        assert_eq!(r.state(), RecorderState::Recording);
        assert!(r.stop().is_none());
        r.reset();
        assert_eq!(r.state(), RecorderState::Idle);
        let mut r = VoiceOverRecorder::new(48_000, RecorderParams { count_in_beats: 0, gain: 2.0, trim_silence_below_db: None, ..Default::default() });
        r.start();
        r.feed(&[0.3, -0.6, 0.1]);
        assert!(r.meter().clipped, "gain 2 clips −0.6");
        let t = r.stop().unwrap();
        assert_eq!(t.samples, vec![0.6, -1.0, 0.2]);
    }
}
