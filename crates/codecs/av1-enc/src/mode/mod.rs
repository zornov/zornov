// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M6: mode decision for intra frames at a fixed quantiser. [`FixedDecider`] makes no choices
//! (fixed partition, `DC_PRED`); [`SearchDecider`] keeps the partition and searches the 13 intra
//! modes per block. Both quantise the residual with M3, reconstruct with M5 as they go, and
//! return what M4's `TileWriter::write_sb` consumes.
//!
//! Limits of this version: 64x64 superblocks, one transform per plane per block (the largest the
//! block allows, `TX_MODE_LARGEST`), luma `DCT_DCT`, no angle deltas, and `base_q_idx > 0`.

pub mod intra;
pub mod partition;
pub mod rdcost;

use crate::recon::FrameBuf;
use crate::types::{Frame, SbDecision, SbPos};

use partition::fixed_partition;

/// Superblock size in 4x4 units.
const SB4: usize = 16;

pub trait ModeDecider {
    /// Chooses the partition, modes and quantised coefficients of one superblock, reconstructing
    /// into `recon` as it goes. Starts the superblock in `recon` (`FrameBuf::begin_sb`); call it
    /// for the superblocks of a frame in raster order, with `recon.quant` set to the frame's
    /// quantiser.
    fn decide_sb(&mut self, src: &Frame, recon: &mut FrameBuf, sb: SbPos) -> SbDecision;
}

fn decide(base_q_idx: u8, search: bool, src: &Frame, recon: &mut FrameBuf, sb: SbPos) -> SbDecision {
    assert_eq!(recon.quant.base_q_idx, base_q_idx, "recon.quant must carry the decider's base_q_idx");
    assert_eq!((recon.width, recon.height), (src.width, src.height), "source and reconstruction differ in size");
    let (mi_cols, mi_rows) = (2 * ((src.width as usize + 7) >> 3), 2 * ((src.height as usize + 7) >> 3));
    let mut shapes = Vec::new();
    fixed_partition(sb.row as usize * SB4, sb.col as usize * SB4, SB4, mi_rows, mi_cols, &mut shapes);
    recon.begin_sb(sb);
    let mut blocks = Vec::with_capacity(shapes.len());
    for shape in shapes {
        let (y_mode, uv_mode) = if search { intra::search_modes(src, recon, shape) } else { (studio_av1::spec_tables::DC_PRED, studio_av1::spec_tables::DC_PRED) };
        blocks.push(intra::code_block(src, recon, shape, y_mode, uv_mode));
    }
    SbDecision { blocks }
}

/// Intra mode search with a partition search: every superblock is split recursively (down to 8x8)
/// where that costs less than coding the block whole.
pub struct PartitionDecider {
    pub base_q_idx: u8,
}

impl ModeDecider for PartitionDecider {
    fn decide_sb(&mut self, src: &Frame, recon: &mut FrameBuf, sb: SbPos) -> SbDecision {
        assert_eq!(recon.quant.base_q_idx, self.base_q_idx, "recon.quant must carry the decider's base_q_idx");
        let (mi_cols, mi_rows) = (2 * ((src.width as usize + 7) >> 3), 2 * ((src.height as usize + 7) >> 3));
        recon.begin_sb(sb);
        let mut blocks = Vec::new();
        partition::search_partition(src, recon, sb.row as usize * SB4, sb.col as usize * SB4, SB4, mi_rows, mi_cols, true, &mut blocks);
        SbDecision { blocks }
    }
}

/// Fixed decisions: no search.
pub struct FixedDecider {
    pub base_q_idx: u8,
}

impl ModeDecider for FixedDecider {
    fn decide_sb(&mut self, src: &Frame, recon: &mut FrameBuf, sb: SbPos) -> SbDecision {
        decide(self.base_q_idx, false, src, recon, sb)
    }
}

/// Intra mode search: per block, the 13 modes are compared by the SATD of the prediction residual
/// plus an estimate of the mode's bits.
pub struct SearchDecider {
    pub base_q_idx: u8,
}

impl ModeDecider for SearchDecider {
    fn decide_sb(&mut self, src: &Frame, recon: &mut FrameBuf, sb: SbPos) -> SbDecision {
        decide(self.base_q_idx, true, src, recon, sb)
    }
}
