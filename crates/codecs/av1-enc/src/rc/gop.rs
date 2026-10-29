// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M8 (phase c): key-frame placement: every `keyint` frames, and at scene cuts.
//!
//! A scene cut is a frame that differs from the previous one by far more than motion explains: the
//! mean absolute difference of the 8x8 block means of the luma planes is large. Coding such a frame
//! as an inter frame costs more than a key frame, and a key frame restarts prediction cleanly.

use crate::types::{Frame, FrameType};

/// Mean difference (0..=255) of the 8x8 block means above which a frame is a scene cut.
pub const SCENE_CUT_THRESHOLD: f64 = 28.0;

/// The luma mean of every 8x8 block of the frame (partial blocks at the edges included).
fn block_means(f: &Frame) -> Vec<u16> {
    let (w, h) = (f.width as usize, f.height as usize);
    let (bw, bh) = (w.div_ceil(8), h.div_ceil(8));
    let mut out = Vec::with_capacity(bw * bh);
    for by in 0..bh {
        for bx in 0..bw {
            let (mut sum, mut n) = (0u32, 0u32);
            for y in by * 8..(by * 8 + 8).min(h) {
                for x in bx * 8..(bx * 8 + 8).min(w) {
                    sum += f.planes[0][y * f.stride[0] + x] as u32;
                    n += 1;
                }
            }
            out.push((sum / n.max(1)) as u16);
        }
    }
    out
}

/// Mean absolute difference of two equally sized block-mean grids, scaled to 8 bits.
fn mean_diff(a: &[u16], b: &[u16], bit_depth: u8) -> f64 {
    let shift = bit_depth.saturating_sub(8) as u32;
    a.iter().zip(b).map(|(&x, &y)| (x as i32 - y as i32).unsigned_abs() as f64).sum::<f64>() / a.len() as f64 / (1u32 << shift) as f64
}

pub struct GopPlanner {
    keyint: u32,
    since_key: Option<u32>,
    prev: Option<Vec<u16>>,
}

impl GopPlanner {
    /// `keyint` is the distance between key frames in frames; 0 means only the first frame (and
    /// scene cuts) are key frames.
    pub fn new(keyint: u32) -> GopPlanner {
        GopPlanner { keyint, since_key: None, prev: None }
    }

    /// The type of the next frame, given the frame itself.
    pub fn next_type(&mut self, frame: &Frame) -> FrameType {
        let means = block_means(frame);
        let cut = match &self.prev {
            Some(p) if p.len() == means.len() => mean_diff(p, &means, frame.bit_depth) > SCENE_CUT_THRESHOLD,
            _ => false,
        };
        self.prev = Some(means);
        let key = match self.since_key {
            None => true,
            Some(n) => cut || (self.keyint != 0 && n + 1 >= self.keyint),
        };
        self.since_key = Some(if key { 0 } else { self.since_key.unwrap_or(0) + 1 });
        if key { FrameType::Key } else { FrameType::Inter }
    }
}
