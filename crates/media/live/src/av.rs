// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pictures and sound to H.264 and AAC, packet by packet: what the file
//! recorder, the replay buffer and the streamers share.

use crate::frame::Frame;
use studio_encode::{aac, h264};

/// One coded packet.
#[derive(Clone, Debug)]
pub enum Packet {
    /// AVCC framed H.264 access unit.
    Video { data: Vec<u8>, keyframe: bool, pts_us: i64 },
    /// One raw AAC frame.
    Audio { data: Vec<u8>, pts_us: i64 },
}

impl Packet {
    pub fn pts_us(&self) -> i64 {
        match self {
            Packet::Video { pts_us, .. } | Packet::Audio { pts_us, .. } => *pts_us,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AvConfig {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub video_kbps: u32,
    pub sample_rate: u32,
    pub audio_kbps: u32,
    /// Seconds between keyframes.
    pub gop_secs: f64,
    /// Constant bit rate (streaming) instead of variable (files).
    pub cbr: bool,
}

impl AvConfig {
    pub fn new(width: u32, height: u32, fps: f64) -> AvConfig {
        let (fps_num, fps_den) = fps_fraction(fps);
        AvConfig { width: width & !1, height: height & !1, fps_num, fps_den, video_kbps: h264::recommended_kbps(width, height, fps).min(12_000), sample_rate: 48_000, audio_kbps: 160, gop_secs: 2.0, cbr: false }
    }

    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den as f64
    }
}

/// 29.97 → 30000/1001 and friends; whole numbers stay whole.
pub fn fps_fraction(fps: f64) -> (u32, u32) {
    for (n, d) in [(24000, 1001), (30000, 1001), (60000, 1001)] {
        if (fps - n as f64 / d as f64).abs() < 0.006 {
            return (n, d);
        }
    }
    (fps.round().max(1.0) as u32, 1)
}

pub struct AvEncoder {
    pub cfg: AvConfig,
    venc: h264::Encoder,
    aenc: aac::Encoder,
    frames: u64,
    pending_audio: Vec<f32>,
    audio_frames_out: u64,
}

impl AvEncoder {
    pub fn new(cfg: AvConfig) -> Result<AvEncoder, String> {
        let mut vc = h264::Config::new(cfg.width, cfg.height, cfg.fps_num, cfg.fps_den);
        vc.bitrate_kbps = cfg.video_kbps;
        vc.gop = (cfg.gop_secs * cfg.fps()).round().max(1.0) as u32;
        // Live and recording both want a steady rate.
        vc.rate_mode = if cfg.cbr { h264::RateMode::Cbr } else { h264::RateMode::Vbr };
        let venc = h264::Encoder::new(vc)?;
        let aenc = aac::Encoder::new(aac::Config { sample_rate: cfg.sample_rate, channels: 2, bitrate: cfg.audio_kbps * 1000 })?;
        Ok(AvEncoder { cfg, venc, aenc, frames: 0, pending_audio: Vec::new(), audio_frames_out: 0 })
    }

    pub fn video_info(&self) -> h264::TrackInfo {
        self.venc.track_info()
    }

    pub fn audio_info(&self) -> aac::TrackInfo {
        self.aenc.track_info()
    }

    pub fn sps(&self) -> &[u8] {
        self.venc.sps()
    }

    pub fn pps(&self) -> &[u8] {
        self.venc.pps()
    }

    /// Makes the next picture a keyframe (a new viewer joined; a replay
    /// clip starts here).
    pub fn force_keyframe(&mut self) {
        self.venc.force_keyframe();
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn push_frame(&mut self, f: &Frame) -> Packet {
        let pts_us = (self.frames as f64 * 1e6 / self.cfg.fps()).round() as i64;
        let fr = if (f.w, f.h) == (self.cfg.width, self.cfg.height) { self.venc.encode_rgba(&f.rgba, f.w, f.h) } else {
            let s = f.scaled(self.cfg.width, self.cfg.height);
            self.venc.encode_rgba(&s.rgba, s.w, s.h)
        };
        self.frames += 1;
        Packet::Video { data: fr.data, keyframe: fr.keyframe, pts_us }
    }

    /// Mono samples at the config's rate (centred in both channels).
    pub fn push_audio(&mut self, mono: &[f32]) -> Vec<Packet> {
        let mut st = Vec::with_capacity(mono.len() * 2);
        for &s in mono {
            st.push(s);
            st.push(s);
        }
        self.pending_audio.clear();
        let aus = self.aenc.push(&st);
        self.pack_audio(aus)
    }

    pub fn flush(&mut self) -> Vec<Packet> {
        let aus = self.aenc.flush();
        self.pack_audio(aus)
    }

    fn pack_audio(&mut self, aus: Vec<Vec<u8>>) -> Vec<Packet> {
        aus.into_iter()
            .map(|data| {
                let pts_us = (self.audio_frames_out as f64 * aac::FRAME as f64 * 1e6 / self.cfg.sample_rate as f64).round() as i64;
                self.audio_frames_out += 1;
                Packet::Audio { data, pts_us }
            })
            .collect()
    }

    pub fn audio_samples_in(&self) -> u64 {
        self.aenc.samples_in()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{AudioSource, TestAudio, TestVideo, VideoSource};

    #[test]
    fn packets_are_timed_and_start_on_a_keyframe() {
        let mut cfg = AvConfig::new(320, 180, 30.0);
        cfg.gop_secs = 1.0;
        let mut e = AvEncoder::new(cfg).unwrap();
        let mut v = TestVideo::from_spec("counter", 320, 180, 30.0).unwrap();
        let mut keys = 0;
        for n in 0..65 {
            let f = v.frame_at(n * 33_333).unwrap();
            match e.push_frame(&f) {
                Packet::Video { keyframe, pts_us, data } => {
                    assert!(!data.is_empty());
                    assert_eq!(pts_us, (n as f64 * 1e6 / 30.0).round() as i64);
                    if n == 0 {
                        assert!(keyframe);
                    }
                    keys += keyframe as u32;
                }
                _ => unreachable!(),
            }
        }
        assert!((2..=4).contains(&keys), "keys {keys}");
        let mut a = TestAudio::from_spec("tone:440", 48000).unwrap();
        let mut n = 0;
        for _ in 0..20 {
            n += e.push_audio(&a.read(1600)).len();
        }
        assert!(n >= 25, "{n}");
        assert_eq!(e.audio_info().sample_rate, 48000);
    }

    #[test]
    fn ntsc_rates_are_exact() {
        assert_eq!(fps_fraction(29.97), (30000, 1001));
        assert_eq!(fps_fraction(25.0), (25, 1));
    }
}
