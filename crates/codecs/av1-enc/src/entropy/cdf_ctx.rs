// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M2: the encoder's CDF set, saved and loaded across frames like the decoder's (spec 7.20, 8.3).

use studio_av1::cdf::Cdfs;

pub struct CdfContext {
    pub cdfs: Box<Cdfs>,
}

impl CdfContext {
    /// Default CDFs for `base_q_idx`.
    pub fn new(base_q_idx: u32) -> CdfContext {
        CdfContext { cdfs: Cdfs::new(base_q_idx) }
    }
    /// The set to store in a reference slot at the end of a frame.
    pub fn save(&self) -> Box<Cdfs> {
        self.cdfs.clone()
    }
    /// Restores a saved set as `load_cdfs` does: the adaptation counters restart at zero.
    pub fn load(saved: &Cdfs) -> CdfContext {
        let mut cdfs = Box::new(saved.clone());
        cdfs.clear_counts();
        CdfContext { cdfs }
    }
}
