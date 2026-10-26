// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Apple ProRes (422 Proxy / LT / 422 / HQ, 4444, 4444 XQ) in MOV / MP4,
//! decoded natively by `studio-prores` at full precision: 10-bit 4:2:2
//! or 12-bit 4:4:4 planes go straight to the converter (and the GPU), so
//! graded ProRes keeps its depth instead of passing through 8-bit H.264.
//! ProRes is intra-only, so any frame decodes on its own.
//!
//! The alpha plane of 4444 is decoded but not drawn yet.

use std::path::Path;
use std::sync::Arc;

use crate::colour_space::{ColourDesc, InputColour};
use crate::scene::Micros;
use crate::source::{MediaSource, SourceInfo};
use crate::yuv::{DecodedPicture, PictureRef, PixelData, Planes};

/// Whether a track's sample entry is ProRes.
pub fn is_prores(fourcc: &[u8; 4]) -> bool {
    studio_prores::Profile::from_fourcc(fourcc).is_some()
}

pub struct ProResSource {
    demuxer: studio_mp4::Demuxer,
    track: studio_mp4::Track,
    /// Sample index of each frame, in presentation order.
    order: Vec<usize>,
    times: Vec<f64>,
    info: SourceInfo,
    input: Option<InputColour>,
    buf: Vec<u8>,
}

impl ProResSource {
    pub fn open(path: &Path, input: Option<InputColour>) -> Result<ProResSource, String> {
        let mut demuxer = studio_mp4::open(path).map_err(|e| format!("{}: {e:?}", path.display()))?;
        let track = demuxer.movie().video().ok_or_else(|| format!("{}: no video track", path.display()))?.clone();
        if !is_prores(&track.fourcc) {
            return Err(format!("{}: not ProRes", path.display()));
        }
        let mut frames: Vec<(f64, usize)> = track.samples.iter().enumerate().filter_map(|(i, s)| track.presentation_secs(s).map(|t| (t, i))).collect();
        frames.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if frames.is_empty() {
            return Err(format!("{}: no ProRes frames", path.display()));
        }
        let first = demuxer.read_sample(&track.samples[frames[0].1]).map_err(|e| format!("{e:?}"))?;
        let f = studio_prores::decode_frame(&first).map_err(|e| format!("{}: {e}", path.display()))?;
        let rate = if frames.len() >= 2 {
            let span = frames[frames.len() - 1].0 - frames[0].0;
            (span > 0.0).then(|| (frames.len() - 1) as f64 / span)
        } else {
            None
        };
        let colour = desc_of(&f, input);
        let info = SourceInfo {
            width: f.width,
            height: f.height,
            rotation: track.rotation as u32 % 360,
            frame_rate: rate,
            duration: Some((track.duration_secs * 1e6).round() as Micros),
            frame_count: Some(frames.len() as u64),
            colour,
        };
        Ok(ProResSource { demuxer, order: frames.iter().map(|f| f.1).collect(), times: frames.iter().map(|f| f.0).collect(), track, info, input, buf: Vec::new() })
    }
}

fn desc_of(f: &studio_prores::Frame, input: Option<InputColour>) -> ColourDesc {
    // ProRes uses 0 for "unspecified"; H.273 uses 2.
    let code = |c: u8| if c == 0 { 2 } else { c };
    let d = ColourDesc::from_h273(code(f.color.primaries), code(f.color.transfer), code(f.color.matrix), false, f.bit_depth.max(8));
    input.map_or(d, |i| i.apply(d))
}

impl MediaSource for ProResSource {
    fn info(&self) -> SourceInfo {
        self.info.clone()
    }

    fn frame_index(&self, time: Micros) -> u64 {
        let secs = time as f64 / 1e6;
        let i = self.times.partition_point(|t| *t <= secs + 1e-6);
        i.saturating_sub(1) as u64
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        let Some(&s) = self.order.get(index as usize) else { return false };
        let sample = self.track.samples[s].clone();
        if self.demuxer.read_sample_into(&sample, &mut self.buf).is_err() {
            return false;
        }
        let Ok(f) = studio_prores::decode_frame(&self.buf) else { return false };
        let colour = desc_of(&f, self.input);
        let cw = f.chroma_width();
        let pic = DecodedPicture {
            width: f.width,
            height: f.height,
            data: PixelData::Yuv { y: Planes::U16(f.y), u: Planes::U16(f.cb), v: Planes::U16(f.cr), chroma_width: cw, chroma_height: f.height },
            colour,
        };
        emit(index, Arc::new(pic));
        true
    }
}
