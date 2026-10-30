// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M10 (phase c): tile layout, parallel tile encoding, tile group OBUs.
//!
//! Tiles are independent in the bitstream: each starts from the frame's CDFs, resets the above and
//! left contexts, the loop-restoration references and the delta-LF state, and does not read mode
//! info, motion vectors or pixels from another tile for prediction (the in-loop filters do cross
//! the borders, after all tiles are coded). So the tiles of a frame can be coded concurrently; this
//! module has the layout (spec 5.9.15), the fan-out ([`encode_tiles`]) and the tile group
//! payload (spec 5.11.1, [`tile_group`]).

use crate::obu::frame_header::{TILE_SIZE_BYTES, clamp_tile_log2};
use crate::types::SeqParams;

/// Tile boundaries in units of superblocks.
///
/// Each vector has one more entry than there are tiles in that direction: the start of every tile
/// and, last, the frame's size in superblocks (the spec's `MiColStarts` / `MiRowStarts` in SB units).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileLayout {
    pub col_starts: Vec<u32>,
    pub row_starts: Vec<u32>,
}

/// The extent of one tile in superblocks (end exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRect {
    pub sb_col0: u32,
    pub sb_col1: u32,
    pub sb_row0: u32,
    pub sb_row1: u32,
}

impl TileRect {
    /// The tile in luma samples for superblocks of `sb_px` (64 or 128), clipped to the frame.
    /// Returns `(x0, x1, y0, y1)`.
    pub fn luma(&self, sb_px: u32, width: u32, height: u32) -> (u32, u32, u32, u32) {
        (self.sb_col0 * sb_px, (self.sb_col1 * sb_px).min(width), self.sb_row0 * sb_px, (self.sb_row1 * sb_px).min(height))
    }

    /// The tile in 4x4 mode-info units for superblocks of `sb_px`, clipped to the frame's
    /// `mi_cols` x `mi_rows`. Returns `(mi_col0, mi_col1, mi_row0, mi_row1)`.
    pub fn mi(&self, sb_px: u32, mi_cols: u32, mi_rows: u32) -> (u32, u32, u32, u32) {
        let s = sb_px / 4;
        (self.sb_col0 * s, (self.sb_col1 * s).min(mi_cols), self.sb_row0 * s, (self.sb_row1 * s).min(mi_rows))
    }
}

fn uniform_starts(sb_count: u32, log2: u8) -> Vec<u32> {
    let size = (sb_count + (1 << log2) - 1) >> log2;
    let mut v: Vec<u32> = (0..sb_count).step_by(size.max(1) as usize).collect();
    v.push(sb_count);
    v
}

impl TileLayout {
    /// A uniform layout (spec 5.9.15) for the given log2 tile counts. As in the spec the number
    /// of tiles can be below `1 << log2` when the frame has too few superblocks to fill them.
    pub fn uniform(sb_cols: u32, sb_rows: u32, cols_log2: u8, rows_log2: u8) -> TileLayout {
        TileLayout { col_starts: uniform_starts(sb_cols, cols_log2), row_starts: uniform_starts(sb_rows, rows_log2) }
    }

    /// The layout the frame header writes for a requested `(cols_log2, rows_log2)`: the request is
    /// clamped to what the frame size allows (`obu::frame_header::clamp_tile_log2`), and the
    /// clamped values are returned with the layout (they are what `FrameParams` must carry).
    pub fn for_frame(seq: &SeqParams, cols_log2: u8, rows_log2: u8) -> (TileLayout, u8, u8) {
        let (c, r) = clamp_tile_log2(seq, cols_log2, rows_log2);
        let sb_mi = if seq.use_128x128_superblock { 32 } else { 16 };
        let (mi_cols, mi_rows) = (2 * ((seq.width + 7) >> 3), 2 * ((seq.height + 7) >> 3));
        (TileLayout::uniform(mi_cols.div_ceil(sb_mi), mi_rows.div_ceil(sb_mi), c, r), c, r)
    }

    pub fn cols(&self) -> usize {
        self.col_starts.len() - 1
    }

    pub fn rows(&self) -> usize {
        self.row_starts.len() - 1
    }

    /// Number of tiles (`NumTiles`).
    pub fn count(&self) -> usize {
        self.cols() * self.rows()
    }

    /// Tile `index` in coding order (raster over tile rows then columns, spec `TileNum`).
    pub fn rect(&self, index: usize) -> TileRect {
        let (row, col) = (index / self.cols(), index % self.cols());
        TileRect { sb_col0: self.col_starts[col], sb_col1: self.col_starts[col + 1], sb_row0: self.row_starts[row], sb_row1: self.row_starts[row + 1] }
    }
}

/// `f(0..n)` in index order, spread over up to `threads` workers that pull indices from a shared
/// counter. The result does not depend on the worker count. With the `threads` feature off, or
/// one worker, the calls run in order on the calling thread.
pub fn par_map<T: Send>(n: usize, threads: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let workers = if cfg!(feature = "threads") { threads.min(n) } else { 1 };
    if workers <= 1 {
        return (0..n).map(f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let parts: Vec<Vec<(usize, T)>> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..workers)
            .map(|_| {
                sc.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            return out;
                        }
                        out.push((i, f(i)));
                    }
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("worker thread panicked")).collect()
    });
    let mut slots: Vec<Option<T>> = (0..n).map(|_| None).collect();
    for (i, t) in parts.into_iter().flatten() {
        slots[i] = Some(t);
    }
    slots.into_iter().map(|t| t.expect("every index was evaluated")).collect()
}

/// Codes every tile of `layout` with `code(tile_index, rect)`, concurrently on up to `threads`
/// workers, and returns the results in tile order. The closure must not depend on the order the
/// tiles run in; then the output is the same for any `threads`.
pub fn encode_tiles<T: Send>(layout: &TileLayout, threads: usize, code: impl Fn(usize, TileRect) -> T + Sync) -> Vec<T> {
    par_map(layout.count(), threads, |i| code(i, layout.rect(i)))
}

/// The tile group of a frame OBU, to follow the frame header's `byte_alignment()`: with several
/// tiles, `tile_start_and_end_present_flag = 0` (the group is the whole frame) padded to a byte,
/// then every tile but the last behind its `tile_size_minus_1` (little endian, [`TILE_SIZE_BYTES`]).
/// `tiles` are the tiles' coded bytes (each ending as `TileWriter::finish` leaves it) in tile order.
pub fn tile_group(tiles: &[Vec<u8>]) -> Vec<u8> {
    assert!(!tiles.is_empty(), "a frame has at least one tile");
    let mut out = Vec::with_capacity(tiles.iter().map(|t| t.len() + TILE_SIZE_BYTES).sum::<usize>() + 1);
    if tiles.len() > 1 {
        out.push(0);
    }
    for (i, t) in tiles.iter().enumerate() {
        if i + 1 < tiles.len() {
            assert!(!t.is_empty() && (t.len() as u64 - 1) < 1 << (8 * TILE_SIZE_BYTES), "tile size does not fit tile_size_bytes");
            out.extend_from_slice(&((t.len() - 1) as u32).to_le_bytes());
        }
        out.extend_from_slice(t);
    }
    out
}

/// Copies the mode info of tile `rect` from `src` into `dst`; both cover the whole frame (a tile
/// coder keeps its mode info in frame coordinates). `sb_px` is the superblock size, 64 or 128.
pub fn merge_mode_info(dst: &mut studio_av1::frame::MiInfo, src: &studio_av1::frame::MiInfo, rect: TileRect, sb_px: u32) {
    assert_eq!((dst.cols, dst.rows, dst.stride, dst.ox, dst.oy), (src.cols, src.rows, src.stride, src.ox, src.oy), "mode info of different frames");
    let (c0, c1, r0, r1) = rect.mi(sb_px, src.cols as u32, src.rows as u32);
    let (c0, c1) = (c0 as usize, c1 as usize);
    macro_rules! copy {
        ($($f:ident)*) => {$(
            if !src.$f.is_empty() {
                for r in r0 as usize..r1 as usize {
                    let i = r * src.stride;
                    dst.$f[i + c0..i + c1].copy_from_slice(&src.$f[i + c0..i + c1]);
                }
            }
        )*};
    }
    copy!(y_mode uv_mode ref_frame mv is_inter skip_mode skip tx_size inter_tx_size mi_size segment_id delta_lf comp_group_idx compound_idx interp_filter motion_mode tx_type written);
    for k in 0..2 {
        if !src.palette_size[k].is_empty() {
            for r in r0 as usize..r1 as usize {
                let i = r * src.stride;
                dst.palette_size[k][i + c0..i + c1].copy_from_slice(&src.palette_size[k][i + c0..i + c1]);
                dst.palette_colors[k][i + c0..i + c1].copy_from_slice(&src.palette_colors[k][i + c0..i + c1]);
            }
        }
    }
}

/// Copies the samples of tile `rect` (whole superblocks, including the part past the picture edge
/// that blocks reach) from the tile coder's reconstruction `src` into `dst`; both are buffers of
/// the whole frame. `sb_px` is the superblock size, 64 or 128.
pub fn merge_recon(dst: &mut crate::recon::FrameBuf, src: &crate::recon::FrameBuf, rect: TileRect, sb_px: u32) {
    assert_eq!((dst.width, dst.height), (src.width, src.height), "reconstructions of different frames");
    for p in 0..3 {
        let s = if p == 0 { 0 } else { 1 };
        let (x0, x1) = ((rect.sb_col0 * sb_px) as usize >> s, (rect.sb_col1 * sb_px) as usize >> s);
        let (y0, y1) = ((rect.sb_row0 * sb_px) as usize >> s, (rect.sb_row1 * sb_px) as usize >> s);
        let (x1, y1) = (x1.min(dst.planes[p].stride), y1.min(dst.planes[p].rows));
        for y in y0..y1 {
            dst.planes[p].row_mut(y)[x0..x1].copy_from_slice(&src.planes[p].row(y)[x0..x1]);
        }
    }
}
