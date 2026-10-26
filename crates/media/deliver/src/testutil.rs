// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Test helpers: tiny generated programmes.

use studio_encode::h264;
use studio_encode::mux::{Muxer, Options};
use studio_encode::yuv::Yuv420;
use std::path::Path;

/// A video-only piece of `n` frames of a moving gradient, from frame `from`.
pub fn piece(path: &Path, w: u32, h: u32, from: u32, n: u32) {
    let cfg = h264::Config::new(w, h, 25, 1);
    let mut enc = h264::Encoder::new(cfg).unwrap();
    let mut mux = Muxer::create(path, Options::mp4()).unwrap();
    let t = mux.add_video(enc.track_info()).unwrap();
    for f in from..from + n {
        let mut px = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                px[i] = ((x * 4 + f * 6) % 256) as u8;
                px[i + 1] = ((y * 4 + f * 3) % 256) as u8;
                px[i + 2] = (f * 5 % 256) as u8;
            }
        }
        let out = enc.encode(&Yuv420::from_rgba(&px, w, h));
        mux.write_sample(t, &out.data, out.keyframe).unwrap();
    }
    mux.finish().unwrap();
}

