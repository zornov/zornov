// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

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
        unimplemented!("M2")
    }
    pub fn load(saved: &Cdfs) -> CdfContext {
        unimplemented!("M2")
    }
}
