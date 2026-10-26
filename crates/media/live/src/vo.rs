// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Voice-over: record the microphone while the picture plays, with a
//! count-in before the take, level metering with peak hold and clip
//! warning, and the take placed where the recording began. The state
//! machine is Studio's own sound recorder (`studio_sound::recorder`); this
//! drives it from a microphone source on the simulated clock (tests, the
//! headless agent) or on the wall clock (the app).

use crate::source::AudioSource;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use studio_sound::recorder::{InputMeter, RecorderParams, RecorderState, Take, VoiceOverRecorder};

#[derive(Clone, Debug)]
pub struct VoSpec {
    /// Timeline seconds where the take starts (where the picture was when
    /// recording began).
    pub start_s: f64,
    /// 0, 3 or 4 beats of count-in.
    pub count_in_beats: u32,
    pub bpm: f32,
    /// Input gain 0..2.
    pub gain: f32,
    /// Trim silence below this level from both ends of the take.
    pub trim_db: Option<f32>,
    /// The microphone's delay in milliseconds: the take is moved earlier
    /// by this much, so words land where they were said.
    pub latency_ms: f64,
}

impl Default for VoSpec {
    fn default() -> Self {
        VoSpec { start_s: 0.0, count_in_beats: 3, bpm: 120.0, gain: 1.0, trim_db: Some(-50.0), latency_ms: 0.0 }
    }
}

fn params(spec: &VoSpec) -> RecorderParams {
    RecorderParams { count_in_beats: spec.count_in_beats, count_in_bpm: spec.bpm, start: (spec.start_s * 1e6) as i64, gain: spec.gain, trim_silence_below_db: spec.trim_db }
}

/// What the take came to.
#[derive(Clone, Debug)]
pub struct VoResult {
    pub take: Take,
    pub clipped: bool,
    pub peak_db: f32,
    /// Seconds spent counting in before the take began.
    pub count_in_s: f64,
}

/// Applies the microphone delay: samples are dropped from the start and
/// the take moves earlier (it cannot start before zero).
fn compensate(mut t: Take, latency_ms: f64) -> Take {
    if latency_ms <= 0.0 {
        return t;
    }
    let drop = ((latency_ms / 1000.0 * t.rate as f64) as usize).min(t.samples.len().saturating_sub(1));
    t.samples.drain(..drop);
    t.duration = (t.samples.len() as f64 / t.rate as f64 * 1e6) as i64;
    t
}

/// Records `seconds` of the microphone after the count-in (simulated
/// clock: as fast as the source delivers). `on_click` gets the stereo
/// count-in clicks block by block, for a monitor output.
pub fn record(mic: &mut dyn AudioSource, spec: &VoSpec, seconds: f64, on_click: &mut dyn FnMut(&[f32])) -> Result<VoResult, String> {
    let rate = mic.rate();
    let mut rec = VoiceOverRecorder::new(rate, params(spec));
    rec.start();
    let block = (rate / 100) as usize;
    let total_record = (seconds * rate as f64) as usize;
    let mut count_in = 0usize;
    let mut clicks = vec![0.0f32; block * 2];
    let mut max_hold = f32::NEG_INFINITY;
    let mut clipped = false;
    // Hard stop so a broken source cannot loop forever.
    for _ in 0..((seconds + 30.0) * 100.0) as usize {
        let b = mic.read(block);
        let was_counting = matches!(rec.state(), RecorderState::CountIn { .. });
        rec.feed(&b);
        if was_counting {
            clicks.fill(0.0);
            rec.render_clicks(&mut clicks);
            on_click(&clicks);
            count_in += block;
        }
        max_hold = max_hold.max(rec.meter().peak_db);
        clipped |= rec.meter().clipped;
        if rec.state() == RecorderState::Recording && rec.recorded_frames() >= total_record {
            break;
        }
    }
    let take = rec.stop().ok_or("nothing was recorded: the microphone gave only silence")?;
    if take.peak < 0.001 {
        return Err("nothing was recorded: the microphone gave only silence (is it muted, or the wrong input?)".into());
    }
    let take = compensate(take, spec.latency_ms);
    let mut take = take;
    if spec.latency_ms > 0.0 {
        take.start = (take.start - (spec.latency_ms * 1000.0) as i64).max(0);
    }
    Ok(VoResult { peak_db: max_hold, clipped, take, count_in_s: count_in as f64 / rate as f64 })
}

/// A voice-over following the wall clock on its own thread.
pub struct LiveVo {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<(RecorderState, InputMeter, usize)>>,
    join: Option<std::thread::JoinHandle<Result<VoResult, String>>>,
}

impl LiveVo {
    pub fn start(mut mic: Box<dyn AudioSource>, spec: VoSpec, mut on_click: Box<dyn FnMut(&[f32]) + Send>) -> LiveVo {
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new((RecorderState::Idle, InputMeter::default(), 0usize)));
        let (s2, st2) = (stop.clone(), state.clone());
        let join = std::thread::spawn(move || {
            let rate = mic.rate();
            let mut rec = VoiceOverRecorder::new(rate, params(&spec));
            rec.start();
            let t0 = std::time::Instant::now();
            let mut fed = 0u64;
            let mut count_in = 0u64;
            let mut clipped = false;
            let mut hold = f32::NEG_INFINITY;
            let mut clicks = Vec::new();
            while !s2.load(Ordering::Relaxed) {
                let due = (t0.elapsed().as_secs_f64() * rate as f64) as u64;
                if due > fed {
                    let n = (due - fed) as usize;
                    let b = mic.read(n);
                    let was = matches!(rec.state(), RecorderState::CountIn { .. });
                    rec.feed(&b);
                    fed += n as u64;
                    if was {
                        clicks.clear();
                        clicks.resize(n * 2, 0.0);
                        rec.render_clicks(&mut clicks);
                        on_click(&clicks);
                        count_in += n as u64;
                    }
                    clipped |= rec.meter().clipped;
                    hold = hold.max(rec.meter().peak_db);
                    if let Ok(mut g) = st2.lock() {
                        *g = (rec.state(), *rec.meter(), rec.recorded_frames());
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(3));
                }
            }
            let mut take = compensate(rec.stop().ok_or("nothing was recorded")?, spec.latency_ms);
            if spec.latency_ms > 0.0 {
                take.start = (take.start - (spec.latency_ms * 1000.0) as i64).max(0);
            }
            Ok(VoResult { peak_db: hold, clipped, take, count_in_s: count_in as f64 / rate as f64 })
        });
        LiveVo { stop, state, join: Some(join) }
    }

    /// State, meter and seconds recorded so far.
    pub fn status(&self) -> (RecorderState, InputMeter, f64) {
        let g = self.state.lock().map(|g| *g).unwrap_or((RecorderState::Idle, InputMeter::default(), 0));
        (g.0, g.1, g.2 as f64 / 48_000.0)
    }

    pub fn stop(mut self) -> Result<VoResult, String> {
        self.stop.store(true, Ordering::Relaxed);
        self.join.take().ok_or("already stopped")?.join().map_err(|_| "the recorder thread crashed".to_string())?
    }
}

impl Drop for LiveVo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::TestAudio;

    #[test]
    fn count_in_clicks_then_the_take_starts_where_the_picture_was() {
        let mut mic = TestAudio::from_spec("voice@0.6", 48000).unwrap();
        let spec = VoSpec { start_s: 12.5, count_in_beats: 4, bpm: 120.0, trim_db: None, ..Default::default() };
        let mut click_energy = 0.0f32;
        let mut click_blocks = 0;
        let r = record(&mut mic, &spec, 3.0, &mut |c| {
            click_blocks += 1;
            click_energy += c.iter().map(|x| x.abs()).sum::<f32>();
        })
        .unwrap();
        assert!((r.count_in_s - 2.0).abs() < 0.02, "4 beats at 120 bpm: {}", r.count_in_s);
        assert!(click_energy > 5.0 && click_blocks >= 190, "{click_energy} {click_blocks}");
        assert_eq!(r.take.start, 12_500_000);
        assert!((r.take.duration as f64 / 1e6 - 3.0).abs() < 0.05, "{}", r.take.duration);
        assert!(r.peak_db > -9.0 && r.peak_db < 0.0, "{}", r.peak_db);
        assert!(!r.clipped);
        // The count-in was not recorded: the take is the voice that came after it.
        let mut again = TestAudio::from_spec("voice@0.6", 48000).unwrap();
        let _skip = again.read(2 * 48000);
        let want = again.read(1000);
        for (a, b) in r.take.samples.iter().zip(&want) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn clipping_is_flagged_and_gain_is_applied() {
        let mut hot = TestAudio::from_spec("tone:200@1.0", 48000).unwrap();
        let r = record(&mut hot, &VoSpec { count_in_beats: 0, gain: 2.0, ..Default::default() }, 0.5, &mut |_| {}).unwrap();
        assert!(r.clipped);
        assert!(r.take.peak >= 0.999);
        let mut quiet = TestAudio::from_spec("tone:200@0.1", 48000).unwrap();
        let r = record(&mut quiet, &VoSpec { count_in_beats: 0, gain: 1.0, ..Default::default() }, 0.5, &mut |_| {}).unwrap();
        assert!(!r.clipped && r.peak_db < -15.0);
    }

    #[test]
    fn silence_is_an_error_and_latency_moves_the_take_earlier() {
        let mut s = TestAudio::from_spec("silence", 48000).unwrap();
        assert!(record(&mut s, &VoSpec { count_in_beats: 0, ..Default::default() }, 0.5, &mut |_| {}).is_err());
        let mut m = TestAudio::from_spec("tone:300@0.5", 48000).unwrap();
        let r = record(&mut m, &VoSpec { start_s: 5.0, count_in_beats: 0, trim_db: None, latency_ms: 120.0, ..Default::default() }, 1.0, &mut |_| {}).unwrap();
        assert_eq!(r.take.start, 4_880_000);
        assert!((r.take.duration as f64 / 1e6 - 0.88).abs() < 0.01, "{}", r.take.duration);
    }

    #[test]
    fn a_live_take_follows_the_wall_clock() {
        let mic = Box::new(TestAudio::from_spec("voice@0.5", 48000).unwrap());
        let vo = LiveVo::start(mic, VoSpec { count_in_beats: 0, trim_db: None, ..Default::default() }, Box::new(|_| {}));
        std::thread::sleep(std::time::Duration::from_millis(700));
        let (state, meter, secs) = vo.status();
        assert_eq!(state, RecorderState::Recording);
        assert!(secs > 0.4 && meter.hold_db > -20.0, "{secs} {meter:?}");
        let r = vo.stop().unwrap();
        assert!(r.take.duration > 500_000);
    }
}
