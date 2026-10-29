// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M9 (phase c): in-loop filters (deblock, CDEF, loop restoration): parameter search and the
//! filtered reconstruction.
//!
//! The filters themselves are the decoder's ([`studio_av1::PostFilter`]), run on the encoder's
//! reconstruction, so the reference frames equal what a decoder outputs. Tile data does not depend
//! on the filter parameters, so the search runs after the tile is coded and the frame header is
//! written with the winners.

use studio_av1::frame::{FrameBuf as DecBuf, MiInfo};
use studio_av1::spec_tables::{NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE};
use studio_av1::{FrameHeader, PostFilter, SequenceHeader};

use crate::recon::FrameBuf;
use crate::syntax::uv_aware_tx_size;
use crate::types::Frame;

/// Which in-loop filters the encoder may use (all by default; off for A/B measurements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterSet {
    pub deblock: bool,
}

impl Default for FilterSet {
    fn default() -> FilterSet {
        FilterSet { deblock: true }
    }
}

/// The decoder-side view of the sequence header the encoder writes.
pub fn decoder_sequence(payload: &[u8]) -> SequenceHeader {
    SequenceHeader::parse(payload).expect("the encoder's own sequence header parses")
}

/// The parts of a frame header the post-filters read.
pub struct FilterParams {
    pub lf_level: [u32; 2],
}

fn frame_header(width: u32, height: u32, p: &FilterParams) -> FrameHeader {
    let mut fh = FrameHeader::default();
    fh.frame_width = width;
    fh.frame_height = height;
    fh.upscaled_width = width;
    fh.mi_cols = 2 * ((width + 7) >> 3);
    fh.mi_rows = 2 * ((height + 7) >> 3);
    fh.lf.level = [p.lf_level[0], p.lf_level[1], p.lf_level[0], p.lf_level[1]];
    fh
}

/// Fills the loop filter's transform sizes from the mode info: luma per block, chroma for the
/// block that carries it.
fn fill_tx_sizes(pf: &mut PostFilter, mi: &MiInfo) {
    for r in 0..mi.rows {
        for c in 0..mi.cols {
            let i = r * mi.stride + c;
            let ms = mi.mi_size[i] as usize;
            let (w4, h4) = (NUM_4X4_BLOCKS_WIDE[ms] as usize, NUM_4X4_BLOCKS_HIGH[ms] as usize);
            if r % h4 != 0 || c % w4 != 0 {
                continue;
            }
            let tx = mi.tx_size[i] as usize;
            for y in r..(r + h4).min(mi.rows + 16) {
                for x in c..(c + w4).min(mi.cols + 16) {
                    pf.set_lf_tx_size(0, y, x, tx as u8);
                }
            }
            let has_chroma = !(w4 == 1 && c & 1 == 0) && !(h4 == 1 && r & 1 == 0);
            if has_chroma {
                let ctx = uv_aware_tx_size(ms, 1, tx) as u8;
                for y in (r >> 1)..(r >> 1) + (h4 >> 1).max(1) {
                    for x in (c >> 1)..(c >> 1) + (w4 >> 1).max(1) {
                        pf.set_lf_tx_size(1, y, x, ctx);
                        pf.set_lf_tx_size(2, y, x, ctx);
                    }
                }
            }
        }
    }
}

fn to_decoder(buf: &FrameBuf) -> DecBuf {
    DecBuf { planes: buf.planes.clone(), num_planes: 3, subsampling_x: 1, subsampling_y: 1, bit_depth: buf.bit_depth, width: buf.width as usize, height: buf.height as usize }
}

/// `recon` (the reconstruction before filtering) run through the decoder's post-filters.
pub fn apply(seq: &SequenceHeader, recon: &FrameBuf, mi: &MiInfo, p: &FilterParams) -> FrameBuf {
    if p.lf_level == [0, 0] {
        return recon.clone();
    }
    let fh = frame_header(recon.width, recon.height, p);
    let mut pf = PostFilter::new(seq, &fh, to_decoder(recon), mi.clone());
    fill_tx_sizes(&mut pf, mi);
    let out = pf.run();
    let mut filtered = recon.clone();
    filtered.planes = out.planes;
    filtered
}

/// Sum of squared differences between the picture area of `buf` and `src`, all planes.
pub fn sse(buf: &FrameBuf, src: &Frame) -> u64 {
    let mut total = 0u64;
    for p in 0..3 {
        let (w, h) = if p == 0 { (src.width as usize, src.height as usize) } else { ((src.width as usize + 1) >> 1, (src.height as usize + 1) >> 1) };
        for y in 0..h {
            let a = &buf.planes[p].row(y)[..w];
            let b = &src.planes[p][y * src.stride[p]..y * src.stride[p] + w];
            total += a.iter().zip(b).map(|(&x, &y)| (x as i64 - y as i64).pow(2) as u64).sum::<u64>();
        }
    }
    total
}

/// The deblocking level (the same for both directions) that minimises the squared error against
/// `src`; 0 when filtering does not help. A ternary search over 0..=63 assumes the error is
/// unimodal in the level, which holds well enough in practice, and a 0 is always compared.
pub fn search_deblock(seq: &SequenceHeader, recon: &FrameBuf, mi: &MiInfo, src: &Frame) -> [u32; 2] {
    let eval = |l: u32| sse(&apply(seq, recon, mi, &FilterParams { lf_level: [l, l] }), src);
    let (mut lo, mut hi) = (1u32, 63u32);
    while hi - lo > 2 {
        let a = lo + (hi - lo) / 3;
        let b = hi - (hi - lo) / 3;
        if eval(a) <= eval(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    let best = (lo..=hi).min_by_key(|&l| eval(l)).unwrap();
    if eval(best) < sse(recon, src) { [best, best] } else { [0, 0] }
}
