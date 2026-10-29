// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Types shared between the modules. Changing this file is a cross-module decision: it is owned by
//! the core worktree.
//!
//! Indices (`TxSize`, `TxType`, `IntraMode`, ...) are the specification's numeric values, the same
//! ones as the named constants in `studio_av1::spec_tables`.

/// A planar frame. Samples are `u16` at every bit depth (8-bit pictures use the low 8 bits),
/// like `studio_av1::Picture`. Chroma planes are `((width + 1) >> 1) x ((height + 1) >> 1)` (4:2:0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub planes: [Vec<u16>; 3],
    pub stride: [usize; 3],
}

impl Frame {
    /// A zeroed 4:2:0 frame.
    pub fn new(width: u32, height: u32, bit_depth: u8) -> Frame {
        let (cw, ch) = ((width as usize + 1) >> 1, (height as usize + 1) >> 1);
        let (w, h) = (width as usize, height as usize);
        Frame { width, height, bit_depth, planes: [vec![0; w * h], vec![0; cw * ch], vec![0; cw * ch]], stride: [w, cw, cw] }
    }
}

/// How the encoder spends bits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RateMode {
    /// Constant quantiser index (0..=255).
    Cqp(u8),
    /// Constant quality: a quality target the rate control turns into per-frame quantisers.
    Crf(u8),
    /// Average bitrate in kilobits per second (1-pass).
    Bitrate(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    /// Frame rate as numerator / denominator.
    pub fps: (u32, u32),
    /// 8 or 10.
    pub bit_depth: u8,
    pub rate: RateMode,
    /// Distance between key frames, in frames.
    pub keyint: u32,
    /// 0 (slowest, best) to 10 (fastest).
    pub speed: u8,
    /// Worker threads (1 = single-threaded, 0 = all cores).
    pub threads: usize,
    /// Tile columns and rows (log2 of the count).
    pub tiles: (u8, u8),
}

impl Config {
    pub fn new(width: u32, height: u32) -> Config {
        Config { width, height, fps: (30, 1), bit_depth: 8, rate: RateMode::Cqp(100), keyint: 240, speed: 6, threads: 1, tiles: (0, 0) }
    }
}

/// One encoded temporal unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// Low-overhead-format OBUs for the MP4 sample: no temporal delimiter, a sequence header only
    /// on key frames.
    pub data: Vec<u8>,
    /// A sync sample: a shown key frame preceded by a sequence header.
    pub keyframe: bool,
    pub pts: i64,
}

/// `cdef_params()`: the strengths a frame's 64x64 blocks pick from (index `cdef_idx`). A secondary
/// strength of 4 is coded as 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdefParams {
    /// 3 to 6.
    pub damping: u8,
    /// log2 of the number of strength sets, 0 to 3.
    pub bits: u8,
    pub y_pri: [u8; 8],
    pub y_sec: [u8; 8],
    pub uv_pri: [u8; 8],
    pub uv_sec: [u8; 8],
}

impl Default for CdefParams {
    fn default() -> CdefParams {
        CdefParams { damping: 3, bits: 0, y_pri: [0; 8], y_sec: [0; 8], uv_pri: [0; 8], uv_sec: [0; 8] }
    }
}

/// Sequence-level parameters (spec 5.5), what `write_sequence_header` and `av1c` need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeqParams {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub fps: (u32, u32),
    /// `seq_profile` (0 = 4:2:0 / monochrome up to 10-bit).
    pub profile: u8,
    pub level_idx: u8,
    pub use_128x128_superblock: bool,
    pub enable_cdef: bool,
    pub enable_restoration: bool,
    pub enable_order_hint: bool,
    /// Colour description (H.273 code points), matching the container's `colr`.
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub full_range: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Key,
    Inter,
    IntraOnly,
}

/// Frame-level parameters (spec 5.9) chosen by the encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameParams {
    pub frame_type: FrameType,
    pub show_frame: bool,
    pub base_q_idx: u8,
    pub disable_cdf_update: bool,
    /// Which of the 8 reference slots this frame refreshes (bit mask).
    pub refresh_frame_flags: u8,
    pub order_hint: u32,
    /// `ref_frame_idx` for LAST..ALTREF (inter frames).
    pub ref_frame_idx: [u8; 7],
    pub loop_filter_level: [u8; 2],
    pub cdef_enabled: bool,
    pub cdef: CdefParams,
    pub tile_cols_log2: u8,
    pub tile_rows_log2: u8,
}

/// `TxSize`, `TxType`, `IntraMode`: the specification's numeric values.
pub type TxSize = usize;
pub type TxType = usize;
pub type IntraMode = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantParams {
    pub base_q_idx: u8,
    pub bit_depth: u8,
    /// Plane: 0 luma, 1 or 2 chroma.
    pub plane: u8,
}

/// Quantised coefficients of one transform block, in the specification's scan layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoefBlock {
    pub plane: u8,
    pub tx_size: TxSize,
    pub tx_type: TxType,
    /// Quantised levels, `levels[pos]` for scan position order; `eob` is the count to write.
    pub levels: Vec<i32>,
    pub eob: usize,
}

/// Position of a superblock in units of superblocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SbPos {
    pub col: u32,
    pub row: u32,
}

/// What the mode decision chose for one block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDecision {
    /// Position in 4x4 units and size in 4x4 units.
    pub mi_row: u32,
    pub mi_col: u32,
    pub w4: u32,
    pub h4: u32,
    pub y_mode: IntraMode,
    pub uv_mode: IntraMode,
    pub tx_size: TxSize,
    pub skip: bool,
    pub coefs: Vec<CoefBlock>,
}

/// What the mode decision chose for one superblock: partition tree flattened to blocks in decode
/// order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SbDecision {
    pub blocks: Vec<BlockDecision>,
}

/// An inter-predicted block of an inter frame (phase (b)): one reference frame, translation only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterDecision {
    /// Position in 4x4 units and size in 4x4 units; at least 2x2 (8x8 samples).
    pub mi_row: u32,
    pub mi_col: u32,
    pub w4: u32,
    pub h4: u32,
    /// `NEARESTMV`, `NEARMV`, `GLOBALMV` (a zero vector, there is no global motion) or `NEWMV`:
    /// the specification's numeric values (`studio_av1::spec_tables`).
    pub mode: usize,
    /// The reference frame, `LAST_FRAME` (1) to `ALTREF_FRAME` (7).
    pub ref_frame: u8,
    /// The candidate the mode picks from the motion vector stack: 0 for `NEARESTMV`, 1 to 3 for
    /// `NEARMV`, 0 to 2 for `NEWMV` (which codes the difference to that candidate).
    pub ref_mv_idx: usize,
    /// The motion vector (row, col) in 1/8 sample units; the writer checks it against what the
    /// mode derives from the stack.
    pub mv: [i32; 2],
    pub skip: bool,
    /// Transform blocks in decode order, as for [`BlockDecision::coefs`]; ignored when `skip`.
    pub coefs: Vec<CoefBlock>,
}

/// One block of an inter frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockMode {
    Intra(BlockDecision),
    Inter(InterDecision),
}

impl BlockMode {
    /// `(mi_row, mi_col, w4, h4)`.
    pub fn shape(&self) -> (usize, usize, usize, usize) {
        match self {
            BlockMode::Intra(b) => (b.mi_row as usize, b.mi_col as usize, b.w4 as usize, b.h4 as usize),
            BlockMode::Inter(b) => (b.mi_row as usize, b.mi_col as usize, b.w4 as usize, b.h4 as usize),
        }
    }
}

/// What the mode decision chose for one superblock of an inter frame, blocks in decode order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InterSbDecision {
    pub blocks: Vec<BlockMode>,
}
