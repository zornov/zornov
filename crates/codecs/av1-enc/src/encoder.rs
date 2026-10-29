// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! Facade: the public encoder. Owns the module pipeline.

use crate::error::{Error, Result};
use crate::types::{Config, Frame, Packet};

pub struct Encoder {
    cfg: Config,
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder> {
        if cfg.width == 0 || cfg.height == 0 {
            return Err(Error::Invalid("zero size"));
        }
        if !matches!(cfg.bit_depth, 8 | 10) {
            return Err(Error::Invalid("bit depth must be 8 or 10"));
        }
        Err(Error::Unsupported("the encoder is not built yet"))
    }
    /// The `av1C` record for the MP4 track.
    pub fn av1c(&self) -> Vec<u8> {
        unimplemented!("facade")
    }
    pub fn push_frame(&mut self, frame: &Frame, pts: i64) -> Result<Vec<Packet>> {
        unimplemented!("facade")
    }
    pub fn flush(&mut self) -> Result<Vec<Packet>> {
        unimplemented!("facade")
    }
}
