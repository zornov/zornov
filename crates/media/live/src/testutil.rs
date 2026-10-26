// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Test helpers: decode a recorded file back to frames with Studio's own
//! demuxer and decoders.

use crate::frame::Frame;
use std::path::Path;

fn to_frame(f: studio_h264::Frame) -> Frame {
    let rgba = studio_h264::to_rgba(&f);
    Frame { w: f.width, h: f.height, rgba, pts_us: 0 }
}

/// Every video frame of an `.mp4`, in display order.
pub fn decode_video(path: &Path) -> Vec<Frame> {
    let mut d = studio_mp4::open(path).unwrap();
    let movie = d.movie().clone();
    let v = movie.video().expect("a video track");
    let studio_mp4::Codec::Avc { config } = &v.codec else { panic!("not H.264") };
    let (sps, pps) = (&config.sps[0], &config.pps[0]);
    let mut avcc = vec![1, config.profile_idc, config.profile_compatibility, config.level_idc, 0xFF, 0xE1];
    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(sps);
    avcc.push(1);
    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(pps);
    let mut dec = studio_h264::Decoder::from_avcc_bytes(&avcc).unwrap();
    let mut out = Vec::new();
    for s in &v.samples {
        let data = d.read_sample(s).unwrap();
        out.extend(dec.decode(&data).unwrap().into_iter().map(to_frame));
    }
    out.extend(dec.flush().into_iter().map(to_frame));
    out
}
