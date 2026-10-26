// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bit-exact comparison against Apple's decode of the fixture streams.

mod common;

use common::compare_with_reference;

#[test]
fn baseline_cavlc_bit_exact() {
    compare_with_reference("h264_baseline_cavlc.mp4", "h264_baseline_cavlc.yuv", 320, 240, 36);
}

#[test]
fn main_cabac_bframes_bit_exact() {
    compare_with_reference("h264_main_cabac_bframes.mp4", "h264_main_cabac_bframes.yuv", 320, 240, 36);
}

#[test]
fn main_cavlc_bframes_bit_exact() {
    compare_with_reference("h264_main_cavlc_bframes.mp4", "h264_main_cavlc_bframes.yuv", 352, 288, 30);
}

#[test]
fn high_cabac_bit_exact() {
    compare_with_reference("h264_high_cabac.mp4", "h264_high_cabac.yuv", 640, 360, 30);
}

#[test]
fn high_odd_size_bit_exact() {
    compare_with_reference("h264_high_odd_size.mov", "h264_high_odd_size.yuv", 318, 182, 24);
}
