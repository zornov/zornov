// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The replay buffer: the last stretch of the program kept as coded
//! packets, ready to save as a clip ("save the last 20 seconds") or to
//! play back as a source ("replay at half speed"). It always starts a
//! saved clip on a keyframe, so a clip is a little longer than asked,
//! never a little shorter.

use crate::av::{AvConfig, Packet};
use crate::frame::Frame;
use crate::sink::Mp4Sink;
use crate::source::VideoSource;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

pub struct ReplayBuffer {
    cfg: AvConfig,
    max_us: i64,
    packets: VecDeque<Packet>,
    avcc: Vec<u8>,
}

impl ReplayBuffer {
    /// Keeps about `max_secs` of program.
    pub fn new(cfg: AvConfig, avcc: Vec<u8>, max_secs: f64) -> ReplayBuffer {
        ReplayBuffer { cfg, max_us: (max_secs * 1e6) as i64, packets: VecDeque::new(), avcc }
    }

    pub fn push(&mut self, p: Packet) {
        self.packets.push_back(p);
        let newest = self.packets.back().map(Packet::pts_us).unwrap_or(0);
        // Drop from the front while the *second* keyframe is still older
        // than the window: the first stays as the start of what is kept.
        loop {
            let keys: Vec<usize> = self.packets.iter().enumerate().filter(|(_, p)| matches!(p, Packet::Video { keyframe: true, .. })).map(|(i, _)| i).take(2).collect();
            if keys.len() < 2 {
                break;
            }
            if newest - self.packets[keys[1]].pts_us() >= self.max_us {
                self.packets.drain(..keys[1]);
            } else {
                break;
            }
        }
    }

    pub fn seconds(&self) -> f64 {
        match (self.packets.front(), self.packets.back()) {
            (Some(a), Some(b)) => (b.pts_us() - a.pts_us()) as f64 / 1e6,
            _ => 0.0,
        }
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// The packets of the last `secs` seconds, from the keyframe at or
    /// before that moment.
    pub fn last(&self, secs: f64) -> Vec<Packet> {
        let newest = match self.packets.back() {
            Some(p) => p.pts_us(),
            None => return Vec::new(),
        };
        let from = newest - (secs * 1e6) as i64;
        let start = self.packets.iter().enumerate().filter(|(_, p)| matches!(p, Packet::Video { keyframe: true, pts_us, .. } if *pts_us <= from)).map(|(i, _)| i).next_back().or_else(|| self.packets.iter().position(|p| matches!(p, Packet::Video { keyframe: true, .. })));
        let Some(start) = start else { return Vec::new() };
        let t0 = self.packets[start].pts_us();
        self.packets.iter().skip(start).filter(|p| p.pts_us() >= t0 || matches!(p, Packet::Audio { .. })).filter(|p| p.pts_us() >= t0).cloned().collect()
    }

    /// Saves the last `secs` seconds as an `.mp4` (no re-encoding).
    pub fn save(&self, path: &Path, secs: f64) -> Result<(PathBuf, f64), String> {
        let pk = self.last(secs);
        if pk.is_empty() {
            return Err("the replay buffer has no picture yet".into());
        }
        let t0 = pk[0].pts_us();
        let has_audio = pk.iter().any(|p| matches!(p, Packet::Audio { .. }));
        let mut sink = Mp4Sink::create(path, self.cfg, has_audio)?;
        for p in &pk {
            sink.packet(p)?;
        }
        let end = pk.last().map(Packet::pts_us).unwrap_or(t0);
        let (path, _) = sink.finish()?;
        Ok((path, (end - t0) as f64 / 1e6 + 1.0 / self.cfg.fps()))
    }

    /// A source that plays the last `secs` seconds at `speed` (0.5 =
    /// half speed). Time zero of the source is when it is first asked.
    pub fn player(&self, secs: f64, speed: f64) -> Result<ReplayPlayer, String> {
        let pk: Vec<Packet> = self.last(secs).into_iter().filter(|p| matches!(p, Packet::Video { .. })).collect();
        if pk.is_empty() {
            return Err("the replay buffer has no picture yet".into());
        }
        Ok(ReplayPlayer { avcc: self.avcc.clone(), packets: pk, speed: speed.clamp(0.05, 2.0), fps: self.cfg.fps(), size: (self.cfg.width, self.cfg.height), dec: None, decoded: 0, current: None, t0: None })
    }
}

/// Plays replay packets as a picture source (decoding as it goes).
pub struct ReplayPlayer {
    avcc: Vec<u8>,
    packets: Vec<Packet>,
    speed: f64,
    fps: f64,
    size: (u32, u32),
    dec: Option<studio_h264::Decoder>,
    /// Packets fed to the decoder.
    decoded: usize,
    current: Option<Frame>,
    t0: Option<i64>,
}

impl ReplayPlayer {
    pub fn duration_secs(&self) -> f64 {
        self.packets.len() as f64 / self.fps / self.speed
    }

    fn restart(&mut self) -> Result<(), String> {
        self.dec = Some(studio_h264::Decoder::from_avcc_bytes(&self.avcc).map_err(|e| e.to_string())?);
        self.decoded = 0;
        Ok(())
    }

    /// Frame number `n` (0-based) of the clip.
    fn frame(&mut self, n: usize) -> Option<Frame> {
        if self.dec.is_none() || n < self.decoded.saturating_sub(1) {
            self.restart().ok()?;
            self.current = None;
        }
        while self.decoded <= n {
            let Packet::Video { data, .. } = &self.packets[self.decoded] else { unreachable!() };
            let dec = self.dec.as_mut()?;
            let out = dec.decode(data).ok()?;
            self.decoded += 1;
            for f in out {
                let rgba = studio_h264::to_rgba(&f);
                self.current = Some(Frame { w: f.width, h: f.height, rgba, pts_us: 0 });
            }
        }
        self.current.clone()
    }
}

impl VideoSource for ReplayPlayer {
    fn label(&self) -> String {
        format!("replay x{}", self.speed)
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn frame_at(&mut self, pts_us: i64) -> Option<Frame> {
        let t0 = *self.t0.get_or_insert(pts_us);
        let secs = (pts_us - t0) as f64 / 1e6;
        let n = ((secs * self.speed * self.fps).floor() as usize).min(self.packets.len() - 1);
        self.frame(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::av::AvEncoder;
    use crate::source::{TestVideo, VideoSource};

    fn filled(secs: f64, keep: f64) -> ReplayBuffer {
        let mut cfg = AvConfig::new(160, 96, 30.0);
        cfg.gop_secs = 1.0;
        let mut enc = AvEncoder::new(cfg).unwrap();
        let mut rb = ReplayBuffer::new(cfg, enc.video_info().avcc, keep);
        let mut v = TestVideo::from_spec("counter", 160, 96, 30.0).unwrap();
        for n in 0..(secs * 30.0) as i64 {
            rb.push(enc.push_frame(&v.frame_at((n as f64 * 1e6 / 30.0) as i64).unwrap()));
        }
        rb
    }

    #[test]
    fn only_the_window_is_kept_and_clips_start_on_a_keyframe() {
        let rb = filled(20.0, 6.0);
        assert!(rb.seconds() >= 6.0 && rb.seconds() < 8.1, "{}", rb.seconds());
        let clip = rb.last(3.0);
        assert!(matches!(clip[0], Packet::Video { keyframe: true, .. }));
        let span = (clip.last().unwrap().pts_us() - clip[0].pts_us()) as f64 / 1e6;
        assert!((3.0..4.1).contains(&span), "{span}");
    }

    #[test]
    fn a_saved_clip_decodes_to_the_last_frames() {
        let rb = filled(10.0, 8.0);
        let p = std::env::temp_dir().join(format!("studio-live-replay-{}.mp4", std::process::id()));
        let (path, secs) = rb.save(&p, 2.0).unwrap();
        assert!(secs >= 2.0 && secs < 3.2, "{secs}");
        let frames = crate::testutil::decode_video(&path);
        assert!(frames.len() >= 60 && frames.len() <= 100, "{}", frames.len());
        // The last frame is the last one pushed (number 299).
        assert_eq!(TestVideo::read_counter(frames.last().unwrap()), 299);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn playback_runs_at_half_speed() {
        let rb = filled(5.0, 4.0);
        let mut pl = rb.player(2.0, 0.5).unwrap();
        let first = TestVideo::read_counter(&pl.frame_at(1_000_000).unwrap());
        let one_sec_later = TestVideo::read_counter(&pl.frame_at(2_000_000).unwrap());
        // 1 s at half speed = 15 frames of the clip.
        assert_eq!(one_sec_later - first, 15);
        assert!(pl.duration_secs() >= 4.0);
    }
}
