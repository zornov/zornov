// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M4: coefficient writer, the inverse of the decoder's `coeffs` (spec 5.11.39).

use crate::entropy::SymbolEncoder;
use crate::types::CoefBlock;

pub fn write_coefficients(enc: &mut SymbolEncoder, block: &CoefBlock) {
    unimplemented!("M4")
}
