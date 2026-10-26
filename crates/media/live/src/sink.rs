// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An `.mp4` being written from pictures and sound: the encoder pair and
//! the muxer together, used by the recorder, the program recording, the
//! ISO recordings and replay saves.

use crate::av::{AvConfig, AvEncoder, Packet};
use crate::frame::Frame;
use std::path::{Path, PathBuf};
use studio_encode::mux::{Muxer, Options};

pub struct Mp4Sink {
    pub enc: AvEncoder,
    mux: Muxer,
    vt: usize,
    at: Option<usize>,
    audio_samples: u64,
    path: PathBuf,
}

impl Mp4Sink {
    pub fn create(path: &Path, cfg: AvConfig, with_audio: bool) -> Result<Mp4Sink, String> {
        let enc = AvEncoder::new(cfg)?;
        let mut mux = Muxer::create(path, Options { fast_start: false, ..Options::mp4() }).map_err(|e| format!("could not create {}: {e}", path.display()))?;
        let vt = mux.add_video(enc.video_info()).map_err(|e| e.to_string())?;
        let at = if with_audio { Some(mux.add_audio(enc.audio_info()).map_err(|e| e.to_string())?) } else { None };
        Ok(Mp4Sink { enc, mux, vt, at, audio_samples: 0, path: path.to_owned() })
    }

    pub fn video(&mut self, f: &Frame) -> Result<(), String> {
        if let Packet::Video { data, keyframe, .. } = self.enc.push_frame(f) {
            self.mux.write_sample(self.vt, &data, keyframe).map_err(|e| format!("could not write {}: {e}", self.path.display()))?;
        }
        Ok(())
    }

    pub fn audio(&mut self, mono: &[f32]) -> Result<(), String> {
        let Some(at) = self.at else { return Ok(()) };
        self.audio_samples += mono.len() as u64;
        for p in self.enc.push_audio(mono) {
            if let Packet::Audio { data, .. } = p {
                self.mux.write_sample(at, &data, true).map_err(|e| format!("could not write {}: {e}", self.path.display()))?;
            }
        }
        Ok(())
    }

    /// Writes an already coded packet (replay saves).
    pub fn packet(&mut self, p: &Packet) -> Result<(), String> {
        match p {
            Packet::Video { data, keyframe, .. } => self.mux.write_sample(self.vt, data, *keyframe),
            Packet::Audio { data, .. } => match self.at {
                Some(at) => {
                    self.audio_samples += studio_encode::aac::FRAME as u64;
                    self.mux.write_sample(at, data, true)
                }
                None => Ok(()),
            },
        }
        .map_err(|e| format!("could not write {}: {e}", self.path.display()))
    }

    pub fn frames(&self) -> u64 {
        self.enc.frames()
    }

    pub fn finish(mut self) -> Result<(PathBuf, u64), String> {
        if let Some(at) = self.at {
            for p in self.enc.flush() {
                if let Packet::Audio { data, .. } = p {
                    self.mux.write_sample(at, &data, true).map_err(|e| e.to_string())?;
                }
            }
            self.mux.set_audio_samples(at, self.audio_samples);
        }
        let frames = self.enc.frames();
        self.mux.finish().map_err(|e| format!("could not finish {}: {e}", self.path.display()))?;
        Ok((self.path, frames))
    }
}
