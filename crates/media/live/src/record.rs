// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Recording: several inputs at once, each to its own file. A camera or a
//! screen (with its microphone) becomes an `.mp4` (H.264 + AAC); a
//! microphone alone becomes a 24-bit `.wav`. Screen inputs can carry the
//! pointer highlight, and keep the pointer's path beside the video.
//!
//! One clock drives everything: after picture number *n* is written, every
//! sound input has delivered exactly the samples up to *(n+1)/fps*. In
//! tests and in the headless agent the clock is simulated (a minute
//! records in a moment); in the app a thread follows the wall clock.

use crate::av::AvConfig;
use crate::sink::Mp4Sink;
use crate::cursor::{CursorSample, CursorSource, CursorStyle, CursorTrack, highlight};
use crate::frame::{Frame, peak_db, rms_db};
use crate::source::{AudioSource, Consent, VideoSource, open_audio, open_video};
use crate::wavout::WavWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct CursorSetup {
    pub source: Box<dyn CursorSource>,
    pub style: CursorStyle,
}

/// One thing to record.
pub struct InputSpec {
    /// Used in the file name.
    pub label: String,
    pub video: Option<String>,
    pub audio: Option<String>,
    pub cursor: Option<CursorSetup>,
}

pub struct RecordSpec {
    pub name: String,
    pub dir: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub inputs: Vec<InputSpec>,
    pub consent: Consent,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct FileInfo {
    pub label: String,
    pub path: String,
    pub kind: String,
    pub seconds: f64,
    pub frames: u64,
    pub bytes: u64,
    pub cursor_file: Option<String>,
}

/// What the meters read now.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Level {
    pub label: String,
    pub peak_db: f32,
    pub rms_db: f32,
    /// Highest peak over the last 1.5 s.
    pub hold_db: f32,
    /// A sample reached full scale in the last block.
    pub clipped: bool,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Progress {
    pub seconds: f64,
    pub frames: u64,
    pub levels: Vec<Level>,
    /// Real time the encoder is behind the clock (realtime recording).
    pub behind_ms: i64,
}

enum Sink {
    Mp4(Box<Mp4Sink>),
    Wav(WavWriter),
}

struct Input {
    label: String,
    path: PathBuf,
    v: Option<Box<dyn VideoSource>>,
    a: Option<Box<dyn AudioSource>>,
    sink: Sink,
    last: Option<Frame>,
    cursor: Option<(CursorSetup, CursorTrack)>,
    cursor_path: PathBuf,
    audio_read: u64,
    level: Level,
    hold: Vec<(i64, f32)>,
}

pub struct Recorder {
    inputs: Vec<Input>,
    fps: f64,
    rate: u32,
    frame: u64,
    bad: Option<String>,
}

fn clean(s: &str) -> String {
    let c: String = s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    c.trim_matches('-').to_owned()
}

impl Recorder {
    pub fn open(spec: RecordSpec) -> Result<Recorder, String> {
        if spec.inputs.is_empty() {
            return Err("nothing to record: give at least one input".into());
        }
        std::fs::create_dir_all(&spec.dir).map_err(|e| format!("could not make {}: {e}", spec.dir.display()))?;
        let rate = 48_000;
        let mut inputs = Vec::new();
        let mut used: Vec<String> = Vec::new();
        for s in spec.inputs {
            if s.video.is_none() && s.audio.is_none() {
                return Err(format!("input \"{}\" has neither a picture nor a sound source", s.label));
            }
            let mut label = clean(&s.label);
            if label.is_empty() || used.contains(&label) {
                label = format!("{}-{}", if label.is_empty() { "input" } else { &label }, inputs.len() + 1);
            }
            used.push(label.clone());
            let v = s.video.as_deref().map(|id| open_video(id, spec.width, spec.height, spec.fps, spec.consent)).transpose()?;
            let a = s.audio.as_deref().map(|id| open_audio(id, rate, spec.consent)).transpose()?;
            let base = format!("{}-{}", clean(&spec.name), label);
            let (path, sink) = if let Some(v) = &v {
                let (w, h) = (spec.width.min(v.size().0.max(spec.width)), spec.height.min(v.size().1.max(spec.height)));
                let mut cfg = AvConfig::new(w, h, spec.fps);
                cfg.sample_rate = rate;
                let path = unique(&spec.dir, &base, "mp4");
                (path.clone(), Sink::Mp4(Box::new(Mp4Sink::create(&path, cfg, a.is_some())?)))
            } else {
                let path = unique(&spec.dir, &base, "wav");
                let w = WavWriter::create(&path, rate).map_err(|e| format!("could not create {}: {e}", path.display()))?;
                (path, Sink::Wav(w))
            };
            let cursor_path = path.with_extension("cursor.json");
            inputs.push(Input { level: Level { label: label.clone(), peak_db: -120.0, rms_db: -120.0, hold_db: -120.0, clipped: false }, label, path, v, a, sink, last: None, cursor: s.cursor.map(|c| (c, CursorTrack::default())), cursor_path, audio_read: 0, hold: Vec::new() });
        }
        Ok(Recorder { inputs, fps: spec.fps, rate, frame: 0, bad: None })
    }

    pub fn seconds(&self) -> f64 {
        self.frame as f64 / self.fps
    }

    pub fn progress(&self) -> Progress {
        Progress { seconds: self.seconds(), frames: self.frame, levels: self.inputs.iter().filter(|i| i.a.is_some()).map(|i| i.level.clone()).collect(), behind_ms: 0 }
    }

    /// Writes the next picture (and the sound up to its end) of every input.
    pub fn step(&mut self) -> Result<(), String> {
        if let Some(b) = &self.bad {
            return Err(b.clone());
        }
        let t_us = (self.frame as f64 * 1e6 / self.fps).round() as i64;
        let want = (((self.frame + 1) as f64 * self.rate as f64 / self.fps).round()) as u64;
        for inp in &mut self.inputs {
            if let (Some(v), Sink::Mp4(sink)) = (&mut inp.v, &mut inp.sink) {
                let mut f = match v.frame_at(t_us) {
                    Some(f) => f,
                    None => inp.last.clone().unwrap_or_else(|| Frame::new(sink.enc.cfg.width, sink.enc.cfg.height, [0, 0, 0, 255])),
                };
                inp.last = Some(f.clone());
                if let Some((c, track)) = &mut inp.cursor {
                    if let Some(s) = c.source.at(t_us) {
                        track.push(s);
                        highlight(&mut f, &c.style, s, track.last_click_before(t_us).map(|c| t_us - c));
                    }
                }
                sink.video(&f)?;
            }
            if let Some(a) = &mut inp.a {
                let n = want.saturating_sub(inp.audio_read) as usize;
                let block = a.read(n);
                inp.audio_read += block.len() as u64;
                let pk = peak_db(&block);
                inp.hold.push((t_us, pk));
                inp.hold.retain(|(t, _)| t_us - t < 1_500_000);
                inp.level = Level { label: inp.label.clone(), peak_db: pk, rms_db: rms_db(&block), hold_db: inp.hold.iter().map(|(_, p)| *p).fold(-120.0, f32::max), clipped: block.iter().any(|s| s.abs() >= 0.999) };
                match &mut inp.sink {
                    Sink::Mp4(sink) => sink.audio(&block)?,
                    Sink::Wav(w) => w.write(&block).map_err(|e| format!("could not write {}: {e}", inp.path.display()))?,
                }
            }
        }
        self.frame += 1;
        Ok(())
    }

    /// Records `seconds` on the simulated clock.
    pub fn run_for(&mut self, seconds: f64) -> Result<(), String> {
        let n = (seconds * self.fps).round() as u64;
        for _ in 0..n {
            self.step()?;
        }
        Ok(())
    }

    /// Ends the recording: the files are complete and playable.
    pub fn finish(mut self) -> Result<Vec<FileInfo>, String> {
        let secs = self.seconds();
        let mut out = Vec::new();
        for inp in self.inputs.drain(..) {
            let (kind, frames, cursor_file) = match inp.sink {
                Sink::Mp4(sink) => {
                    let (_, frames) = sink.finish()?;
                    ("video", frames, None)
                }
                Sink::Wav(w) => {
                    let frames = w.frames();
                    w.finish().map_err(|e| e.to_string())?;
                    ("audio", frames, None)
                }
            };
            let mut cursor_file = cursor_file;
            if let Some((_, track)) = inp.cursor {
                std::fs::write(&inp.cursor_path, serde_json::to_vec(&track).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                cursor_file = Some(inp.cursor_path.to_string_lossy().into_owned());
            }
            let bytes = std::fs::metadata(&inp.path).map(|m| m.len()).unwrap_or(0);
            out.push(FileInfo { label: inp.label, path: inp.path.to_string_lossy().into_owned(), kind: kind.into(), seconds: secs, frames, bytes, cursor_file });
        }
        Ok(out)
    }
}

fn unique(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let p = dir.join(format!("{base}.{ext}"));
    if !p.exists() {
        return p;
    }
    (2..).map(|n| dir.join(format!("{base}-{n}.{ext}"))).find(|p| !p.exists()).unwrap()
}

/// A recording that follows the wall clock on its own thread.
pub struct LiveRecording {
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<Progress>>,
    join: Option<std::thread::JoinHandle<Result<Vec<FileInfo>, String>>>,
}

impl LiveRecording {
    pub fn start(mut rec: Recorder) -> LiveRecording {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(rec.progress()));
        let (s2, sh2) = (stop.clone(), shared.clone());
        let join = std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            while !s2.load(Ordering::Relaxed) {
                let elapsed = t0.elapsed().as_secs_f64();
                let due = (elapsed * rec.fps).floor() as u64;
                if rec.frame < due {
                    rec.step()?;
                    let mut p = rec.progress();
                    p.behind_ms = ((due - rec.frame) as f64 * 1000.0 / rec.fps) as i64;
                    if let Ok(mut g) = sh2.lock() {
                        *g = p;
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(4));
                }
            }
            rec.finish()
        });
        LiveRecording { stop, shared, join: Some(join) }
    }

    pub fn progress(&self) -> Progress {
        self.shared.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn stop(mut self) -> Result<Vec<FileInfo>, String> {
        self.stop.store(true, Ordering::Relaxed);
        self.join.take().ok_or("already stopped")?.join().map_err(|_| "the recorder thread crashed".to_string())?
    }
}

impl Drop for LiveRecording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A cursor path for tests and the agent's test screens: sweeps across
/// the picture and clicks once per `click_every` seconds.
pub fn demo_cursor(click_every: f64) -> CursorSetup {
    let mut pts = Vec::new();
    let total = 600.0;
    for i in 0..=600 {
        let t = i as f64 * 0.1;
        let x = 0.15 + 0.7 * (0.5 + 0.5 * (t * 0.8).sin());
        let y = 0.2 + 0.6 * (0.5 + 0.5 * (t * 0.5).cos());
        let down = (t % click_every) < 0.2 && t > 0.0;
        pts.push(CursorSample { t_us: (t * 1e6) as i64, x: x as f32, y: y as f32, down });
    }
    let _ = total;
    CursorSetup { source: Box::new(crate::cursor::ScriptedCursor { points: pts }), style: CursorStyle::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-live-rec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn two_cameras_and_a_mic_record_at_once_into_separate_files() {
        let dir = tmp("multi");
        let spec = RecordSpec {
            name: "take1".into(),
            dir: dir.clone(),
            width: 320,
            height: 180,
            fps: 30.0,
            inputs: vec![
                InputSpec { label: "cam a".into(), video: Some("test:counter:1".into()), audio: Some("test:tone:440".into()), cursor: None },
                InputSpec { label: "cam b".into(), video: Some("test:counter:2".into()), audio: None, cursor: None },
                InputSpec { label: "lav".into(), video: None, audio: Some("test:voice".into()), cursor: None },
            ],
            consent: Consent::default(),
        };
        let mut r = Recorder::open(spec).unwrap();
        r.run_for(2.0).unwrap();
        let pr = r.progress();
        assert_eq!(pr.frames, 60);
        assert_eq!(pr.levels.len(), 2);
        let files = r.finish().unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].frames, 60);
        assert!(files[0].path.ends_with("take1-cam-a.mp4"));
        // The files are real: the mp4 demuxes with the right track sizes.
        let m = studio_mp4::open(&files[0].path).unwrap();
        assert_eq!(m.movie().tracks.len(), 2);
        let frames = crate::testutil::decode_video(Path::new(&files[0].path));
        assert_eq!(frames.len(), 60);
        for (n, f) in frames.iter().enumerate().step_by(7) {
            assert_eq!(crate::source::TestVideo::read_counter(f), n as u32, "frame {n}");
        }
        let b = crate::testutil::decode_video(Path::new(&files[1].path));
        assert_eq!(b.len(), 60);
        assert_ne!(b[3].px(300, 100), frames[3].px(300, 100), "two cameras, two pictures");
        let (rate, lav) = crate::wavout::read_mono(Path::new(&files[2].path)).unwrap();
        assert_eq!(rate, 48000);
        assert_eq!(lav.len(), 96_000);
        assert!(peak_db(&lav) > -12.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn screen_with_cursor_highlight_keeps_the_pointer_path() {
        let dir = tmp("screen");
        let spec = RecordSpec { name: "screen".into(), dir: dir.clone(), width: 320, height: 180, fps: 24.0, inputs: vec![InputSpec { label: "display".into(), video: Some("test:grey".into()), audio: None, cursor: Some(demo_cursor(1.0)) }], consent: Consent::default() };
        let mut r = Recorder::open(spec).unwrap();
        r.run_for(3.0).unwrap();
        let files = r.finish().unwrap();
        let track: CursorTrack = serde_json::from_slice(&std::fs::read(files[0].cursor_file.as_ref().unwrap()).unwrap()).unwrap();
        assert_eq!(track.samples.len(), 72);
        assert!(track.clicks.len() >= 2, "{:?}", track.clicks);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn real_devices_are_refused_without_the_persons_action() {
        let dir = tmp("refuse");
        let spec = RecordSpec { name: "x".into(), dir: dir.clone(), width: 320, height: 180, fps: 30.0, inputs: vec![InputSpec { label: "cam".into(), video: Some("camera:default".into()), audio: None, cursor: None }], consent: Consent::default() };
        let e = Recorder::open(spec).err().unwrap();
        assert!(e.contains("real device"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_live_recording_follows_the_wall_clock() {
        let dir = tmp("live");
        let spec = RecordSpec { name: "live".into(), dir: dir.clone(), width: 160, height: 90, fps: 30.0, inputs: vec![InputSpec { label: "a".into(), video: Some("test:bars".into()), audio: Some("test:tone:300".into()), cursor: None }], consent: Consent::default() };
        let live = LiveRecording::start(Recorder::open(spec).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let mid = live.progress();
        assert!(mid.frames > 10, "{mid:?}");
        let files = live.stop().unwrap();
        assert!(files[0].seconds > 0.8 && files[0].seconds < 2.5, "{}", files[0].seconds);
        let _ = std::fs::remove_dir_all(dir);
    }
}
