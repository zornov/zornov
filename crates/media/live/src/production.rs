// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A live production: the vision mixer plus everything it feeds. Each
//! `step` makes one frame of program and sends it to the program
//! recording, to the isolated (ISO) recordings of every source, to the
//! streams, and to the replay buffer.

use crate::av::{AvConfig, AvEncoder, Packet};
use crate::frame::Frame;
use crate::replay::ReplayBuffer;
use crate::rtmp::Meta;
use crate::sink::Mp4Sink;
use crate::source::Consent;
use crate::stream::{Stream, StreamInfo, StreamStatus};
use crate::switcher::Switcher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct SourceSpec {
    pub name: String,
    pub video: String,
    pub audio: Option<String>,
}

pub struct ProductionSpec {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub sources: Vec<SourceSpec>,
    pub consent: Consent,
    /// Seconds of program kept for instant replay (0: no replay buffer).
    pub replay_secs: f64,
}

pub struct Production {
    pub sw: Switcher,
    cfg: AvConfig,
    program_rec: Option<(Mp4Sink, PathBuf)>,
    isos: Vec<(usize, Mp4Sink, PathBuf)>,
    live_enc: Option<AvEncoder>,
    streams: Vec<Stream>,
    pub replay: Option<ReplayBuffer>,
    frames: u64,
    pub last_program: Option<Frame>,
    /// How far the encoder is behind the wall clock (a real-time show on a
    /// machine that cannot keep up), milliseconds.
    pub behind_ms: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ProductionFile {
    pub kind: String,
    pub source: Option<String>,
    pub path: String,
    pub frames: u64,
}

impl Production {
    pub fn open(spec: ProductionSpec) -> Result<Production, String> {
        if spec.sources.is_empty() {
            return Err("a production needs at least one source".into());
        }
        crate::text::warm();
        let mut sw = Switcher::new(spec.width, spec.height, spec.fps, spec.consent);
        for s in &spec.sources {
            sw.add_source(&s.name, &s.video, s.audio.as_deref())?;
        }
        let mut cfg = AvConfig::new(sw.w, sw.h, spec.fps);
        cfg.cbr = true;
        let mut p = Production { sw, cfg, program_rec: None, isos: Vec::new(), live_enc: None, streams: Vec::new(), replay: None, frames: 0, last_program: None, behind_ms: 0 };
        if spec.replay_secs > 0.0 {
            p.ensure_live_encoder()?;
            let avcc = p.live_enc.as_ref().map(|e| e.video_info().avcc).unwrap_or_default();
            p.replay = Some(ReplayBuffer::new(p.cfg, avcc, spec.replay_secs));
        }
        Ok(p)
    }

    fn ensure_live_encoder(&mut self) -> Result<(), String> {
        if self.live_enc.is_none() {
            self.live_enc = Some(AvEncoder::new(self.cfg)?);
        }
        Ok(())
    }

    pub fn seconds(&self) -> f64 {
        self.frames as f64 / self.cfg.fps()
    }

    /// Records the program (picture and sound as switched) to `dir`.
    pub fn record_program(&mut self, dir: &Path, name: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = unique(dir, &format!("{name}-program"));
        let mut cfg = AvConfig::new(self.sw.w, self.sw.h, self.cfg.fps());
        cfg.video_kbps = cfg.video_kbps.max(8000);
        self.program_rec = Some((Mp4Sink::create(&path, cfg, true)?, path.clone()));
        Ok(path)
    }

    /// Records every source on its own (ISO), whatever is on air.
    pub fn record_isos(&mut self, dir: &Path, name: &str) -> Result<Vec<PathBuf>, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut paths = Vec::new();
        let st = self.sw.state();
        for s in &st.sources {
            let path = unique(dir, &format!("{name}-iso-{}", s.name.replace(|c: char| !c.is_alphanumeric(), "-")));
            let cfg = AvConfig::new(self.sw.w, self.sw.h, self.cfg.fps());
            self.isos.push((s.index, Mp4Sink::create(&path, cfg, true)?, path.clone()));
            paths.push(path);
        }
        self.sw.want_iso(true);
        Ok(paths)
    }

    pub fn stream_to(&mut self, url: &str) -> Result<(), String> {
        self.ensure_live_encoder()?;
        let enc = self.live_enc.as_mut().unwrap();
        let info = StreamInfo {
            avcc: enc.video_info().avcc,
            sps: enc.sps().to_vec(),
            pps: enc.pps().to_vec(),
            asc: enc.audio_info().asc,
            meta: Meta { width: self.cfg.width, height: self.cfg.height, fps: self.cfg.fps(), video_kbps: self.cfg.video_kbps, audio_kbps: self.cfg.audio_kbps, sample_rate: self.cfg.sample_rate },
        };
        let s = Stream::start(url, info)?;
        // A new viewer needs a keyframe to begin on.
        enc.force_keyframe();
        self.streams.push(s);
        Ok(())
    }

    pub fn stream_status(&self) -> Vec<StreamStatus> {
        self.streams.iter().map(Stream::status).collect()
    }

    pub fn stop_streams(&mut self) -> Vec<StreamStatus> {
        self.streams.drain(..).map(Stream::stop).collect()
    }

    /// One frame of program, sent everywhere it goes.
    pub fn step(&mut self) -> Result<(), String> {
        let out = self.sw.step();
        if let Some((rec, _)) = &mut self.program_rec {
            rec.video(&out.program)?;
            rec.audio(&out.audio.samples)?;
        }
        for (i, sink, _) in &mut self.isos {
            if let Some((Some(f), a)) = out.iso.get(*i) {
                sink.video(f)?;
                sink.audio(a)?;
            }
        }
        if let Some(enc) = &mut self.live_enc {
            let mut pk: Vec<Packet> = vec![enc.push_frame(&out.program)];
            pk.extend(enc.push_audio(&out.audio.samples));
            for p in &pk {
                for s in &self.streams {
                    s.send(p);
                }
                if let Some(r) = &mut self.replay {
                    r.push(p.clone());
                }
            }
        }
        self.last_program = Some(out.program);
        self.frames += 1;
        Ok(())
    }

    pub fn run_for(&mut self, seconds: f64) -> Result<(), String> {
        for _ in 0..(seconds * self.cfg.fps()).round() as u64 {
            self.step()?;
        }
        Ok(())
    }

    /// Plays the last `secs` seconds back as a new source called `name`, at `speed`.
    pub fn replay_as_source(&mut self, name: &str, secs: f64, speed: f64) -> Result<usize, String> {
        let player = self.replay.as_ref().ok_or("no replay buffer: open the production with replay_secs")?.player(secs, speed)?;
        Ok(self.sw.add_custom(name, Box::new(player)))
    }

    /// Saves the last `secs` seconds of program.
    pub fn save_replay(&self, dir: &Path, name: &str, secs: f64) -> Result<(PathBuf, f64), String> {
        let rb = self.replay.as_ref().ok_or("no replay buffer: start the production with replay_secs")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        rb.save(&unique(dir, &format!("{name}-replay")), secs)
    }

    /// Ends everything: files are finished, streams closed.
    pub fn finish(mut self) -> Result<Vec<ProductionFile>, String> {
        let mut files = Vec::new();
        let names: Vec<String> = self.sw.state().sources.iter().map(|s| s.name.clone()).collect();
        if let Some((rec, path)) = self.program_rec.take() {
            let (_, frames) = rec.finish()?;
            files.push(ProductionFile { kind: "program".into(), source: None, path: path.to_string_lossy().into_owned(), frames });
        }
        for (i, sink, path) in self.isos.drain(..) {
            let (_, frames) = sink.finish()?;
            files.push(ProductionFile { kind: "iso".into(), source: names.get(i).cloned(), path: path.to_string_lossy().into_owned(), frames });
        }
        self.stop_streams();
        Ok(files)
    }
}

fn unique(dir: &Path, base: &str) -> PathBuf {
    let p = dir.join(format!("{base}.mp4"));
    if !p.exists() {
        return p;
    }
    (2..).map(|n| dir.join(format!("{base}-{n}.mp4"))).find(|p| !p.exists()).unwrap()
}

/// A production that follows the wall clock on its own thread; commands
/// reach it through [`LiveProduction::with`].
pub struct LiveProduction {
    p: Arc<Mutex<Option<Production>>>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl LiveProduction {
    pub fn start(prod: Production) -> LiveProduction {
        let p = Arc::new(Mutex::new(Some(prod)));
        let stop = Arc::new(AtomicBool::new(false));
        let (p2, s2) = (p.clone(), stop.clone());
        let join = std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            while !s2.load(Ordering::Relaxed) {
                let mut g = p2.lock().map_err(|_| "poisoned".to_string())?;
                let Some(prod) = g.as_mut() else { break };
                let due = (t0.elapsed().as_secs_f64() * prod.cfg.fps()).floor() as u64;
                if prod.frames < due {
                    prod.step()?;
                    prod.behind_ms = ((due.saturating_sub(prod.frames)) as f64 * 1000.0 / prod.cfg.fps()) as i64;
                    drop(g);
                    // Let a waiting command in (a mutex does not queue fairly).
                    std::thread::sleep(std::time::Duration::from_micros(500));
                } else {
                    drop(g);
                    std::thread::sleep(std::time::Duration::from_millis(3));
                }
            }
            Ok(())
        });
        LiveProduction { p, stop, join: Some(join) }
    }

    /// Runs `f` on the production (between two frames).
    pub fn with<T>(&self, f: impl FnOnce(&mut Production) -> T) -> Result<T, String> {
        let mut g = self.p.lock().map_err(|_| "the production crashed")?;
        g.as_mut().map(f).ok_or_else(|| "the production has ended".to_string())
    }

    pub fn stop(mut self) -> Result<Vec<ProductionFile>, String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            j.join().map_err(|_| "the production thread crashed".to_string())??;
        }
        let prod = self.p.lock().map_err(|_| "poisoned")?.take().ok_or("already stopped")?;
        prod.finish()
    }
}

impl Drop for LiveProduction {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::TestVideo;
    use crate::switcher::{Layout, Transition, TitleKind};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-live-prod-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn spec() -> ProductionSpec {
        ProductionSpec {
            width: 320,
            height: 180,
            fps: 30.0,
            sources: vec![
                SourceSpec { name: "wide".into(), video: "test:counter:1".into(), audio: Some("test:tone:300".into()) },
                SourceSpec { name: "close".into(), video: "test:counter:2".into(), audio: Some("test:tone:700".into()) },
            ],
            consent: Consent::default(),
            replay_secs: 8.0,
        }
    }

    #[test]
    fn a_show_is_switched_recorded_with_isos_and_replayed() {
        let dir = tmp("show");
        let mut p = Production::open(spec()).unwrap();
        p.record_program(&dir, "show").unwrap();
        let isos = p.record_isos(&dir, "show").unwrap();
        assert_eq!(isos.len(), 2);
        p.sw.cut_to(Layout::Full(0)).unwrap();
        p.run_for(2.0).unwrap();
        p.sw.take_layout(Layout::Full(1), Transition::Mix, 500);
        p.sw.title_show(TitleKind::Lower, "Priya Sharma", "Host", 200);
        p.run_for(2.0).unwrap();
        let (clip, secs) = p.save_replay(&dir, "show", 2.0).unwrap();
        assert!(secs >= 2.0, "{secs}");
        p.run_for(1.0).unwrap();
        let files = p.finish().unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|f| f.frames == 150));
        // The program recording shows camera 0 then camera 1 (by the counter's background).
        let prog = crate::testutil::decode_video(Path::new(&files[0].path));
        assert_eq!(prog.len(), 150);
        let bg = |f: &Frame| f.px(5, f.h - 5);
        let (early, late) = (bg(&prog[20]), bg(&prog[140]));
        assert_ne!(early, late, "the picture changed camera");
        // The ISO of the close camera runs the whole time, whatever was on air.
        let iso1 = crate::testutil::decode_video(Path::new(&files[2].path));
        assert_eq!(iso1.len(), 150);
        assert_eq!(bg(&iso1[20]), late, "close-up ISO has the second camera's picture from the start");
        // Both cameras' frame counters agree on time.
        assert_eq!(TestVideo::read_counter(&iso1[100]), 100);
        let rp = crate::testutil::decode_video(&clip);
        assert!(rp.len() >= 60);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_live_production_takes_commands_on_the_wall_clock() {
        let dir = tmp("live");
        let mut prod = Production::open(ProductionSpec { replay_secs: 0.0, ..spec() }).unwrap();
        prod.record_program(&dir, "live").unwrap();
        let live = LiveProduction::start(prod);
        std::thread::sleep(std::time::Duration::from_millis(500));
        live.with(|p| p.sw.cut_to(Layout::Full(1))).unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(700));
        let secs = live.with(|p| p.seconds()).unwrap();
        assert!(secs > 0.3, "{secs}");
        let files = live.stop().unwrap();
        assert_eq!(files.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn streaming_reaches_a_server_and_survives_it_going_away() {
        let (port, server) = crate::rtmp::mock::serve(false);
        let mut p = Production::open(ProductionSpec { replay_secs: 0.0, ..spec() }).unwrap();
        p.stream_to(&format!("rtmp://127.0.0.1:{port}/live/key123")).unwrap();
        p.run_for(2.0).unwrap();
        let st = p.stream_status();
        assert_eq!(st.len(), 1);
        assert!(!st[0].target.contains("key123"));
        let done = p.stop_streams();
        assert_eq!(done[0].state, "stopped");
        assert!(done[0].sent > 60, "{:?}", done[0]);
        let seen = server.join().unwrap();
        assert!(seen.video.len() > 50);
        // A wrong address is an error at once.
        let mut q = Production::open(ProductionSpec { replay_secs: 0.0, ..spec() }).unwrap();
        assert!(q.stream_to("rtmp://127.0.0.1:1/live/x").unwrap_err().contains("could not reach"));
        assert!(q.stream_to("http://example.com/x").unwrap_err().contains("rtmp://"));
    }
}
