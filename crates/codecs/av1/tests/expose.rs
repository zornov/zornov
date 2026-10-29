// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The pieces an encoder reuses are reachable from outside the crate.

use studio_av1::cdf::Cdfs;
use studio_av1::spec_tables::{AC_QLOOKUP, DC_QLOOKUP, DEFAULT_ANGLE_DELTA_CDF};
use studio_av1::symbol::{update_cdf, SymbolDecoder};
use studio_av1::transform::inverse_transform_2d;

#[test]
fn tables_cdfs_symbols_and_transforms_are_reachable() {
    assert_eq!(DC_QLOOKUP[0].len(), 256);
    assert_eq!(AC_QLOOKUP[0].len(), 256);
    assert!(!DEFAULT_ANGLE_DELTA_CDF.is_empty());

    let mut cdfs = Cdfs::new(60);
    let n = cdfs.y_mode[0].len() - 1;
    let before = cdfs.y_mode[0];
    update_cdf(&mut cdfs.y_mode[0], n, 3);
    assert_ne!(before, cdfs.y_mode[0], "adaptation moves the CDF");

    let mut d = SymbolDecoder::new(&[0u8; 8], true);
    let _ = d.read_literal(4);

    // Coefficients are a 64x64 buffer (row stride 64); a DC-only 4x4 block gives a flat residual.
    let mut dequant = vec![0i32; 64 * 64];
    let mut residual = [0i32; 16];
    inverse_transform_2d(&dequant, &mut residual, 0, 0, false, 8);
    assert!(residual.iter().all(|&v| v == 0));
    dequant[0] = 256;
    inverse_transform_2d(&dequant, &mut residual, 0, 0, false, 8);
    assert!(residual[0] != 0 && residual.iter().all(|&v| v == residual[0]), "{residual:?}");
}
