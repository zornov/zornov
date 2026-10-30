// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Core: the skeleton compiles, the shared types behave, and the encoder refuses what it cannot do.

use studio_av1_enc::{Config, Encoder, Error, Frame, RateMode};

#[test]
fn frame_has_420_planes() {
    let f = Frame::new(33, 17, 8);
    assert_eq!(f.planes[0].len(), 33 * 17);
    assert_eq!(f.planes[1].len(), 17 * 9);
    assert_eq!(f.stride, [33, 17, 17]);
}

#[test]
fn config_defaults() {
    let c = Config::new(640, 360);
    assert_eq!((c.fps, c.bit_depth, c.tiles), ((30, 1), 8, (0, 0)));
    assert_eq!(c.rate, RateMode::Cqp(100));
}

#[test]
fn encoder_validates_then_says_unsupported_for_what_is_not_built() {
    assert!(matches!(Encoder::new(Config::new(0, 360)), Err(Error::Invalid(_))));
    let mut c = Config::new(64, 64);
    c.bit_depth = 12;
    assert!(matches!(Encoder::new(c), Err(Error::Invalid(_))));
    // 10-bit is built (its streams are checked in tests/encoder.rs); lossless is a later phase
    c = Config::new(64, 64);
    c.bit_depth = 10;
    assert!(Encoder::new(c).is_ok());
    let mut c = Config::new(64, 64);
    c.rate = RateMode::Cqp(0);
    assert!(matches!(Encoder::new(c), Err(Error::Unsupported(_))));
    assert!(Encoder::new(Config::new(64, 64)).is_ok());
}

#[test]
fn the_decoder_pieces_the_encoder_reuses_are_reachable() {
    let c = studio_av1::cdf::Cdfs::new(60);
    assert!(!c.y_mode.is_empty());
    assert_eq!(studio_av1::spec_tables::DC_QLOOKUP[0].len(), 256);
}
