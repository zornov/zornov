// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M6: mode decision. Phase (a) is fixed (one partition, `DC_PRED`); later it searches.

pub mod intra;
pub mod partition;
pub mod rdcost;

use crate::recon::FrameBuf;
use crate::types::{Frame, SbDecision, SbPos};

pub trait ModeDecider {
    /// Chooses the partition, modes and quantised coefficients of one superblock, reconstructing
    /// into `recon` as it goes.
    fn decide_sb(&mut self, src: &Frame, recon: &mut FrameBuf, sb: SbPos) -> SbDecision;
}

/// Fixed decisions: no search.
pub struct FixedDecider {
    pub base_q_idx: u8,
}

impl ModeDecider for FixedDecider {
    fn decide_sb(&mut self, src: &Frame, recon: &mut FrameBuf, sb: SbPos) -> SbDecision {
        unimplemented!("M6")
    }
}
