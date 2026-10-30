// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M1: tests for the `obu` module: OBU framing, the sequence header against `studio_av1`'s parser
//! (every field read back), the frame header against an independent spec 5.9 reader plus
//! acceptance by a full `studio_av1::Decoder`, and the `av1C` record.

use studio_av1::{Decoder, SequenceHeader};
use studio_av1_enc::bits::BitWriter;
use studio_av1_enc::obu::frame_header::{clamp_tile_log2, write_frame_header};
use studio_av1_enc::obu::seq_header::{ORDER_HINT_BITS, av1c, write_sequence_header};
use studio_av1_enc::obu::{ObuType, frame_header_obu, obu, temporal_delimiter};
use studio_av1_enc::types::{FrameParams, FrameType, SeqParams};

#[path = "bits.rs"]
#[allow(dead_code)]
mod bits_test;
use bits_test::Reader;

fn seq(w: u32, h: u32) -> SeqParams {
    SeqParams {
        width: w,
        height: h,
        bit_depth: 8,
        fps: (30, 1),
        profile: 0,
        level_idx: 8,
        use_128x128_superblock: false,
        enable_cdef: false,
        enable_restoration: false,
        enable_order_hint: true,
        color_primaries: 1,
        transfer_characteristics: 1,
        matrix_coefficients: 1,
        full_range: false,
    }
}

fn key(q: u8) -> FrameParams {
    FrameParams {
        frame_type: FrameType::Key,
        show_frame: true,
        base_q_idx: q,
        disable_cdf_update: false,
        refresh_frame_flags: 0xFF,
        order_hint: 0,
        ref_frame_idx: [0; 7],
        loop_filter_level: [0, 0],
        cdef_enabled: false,
        cdef: Default::default(),
        lr: Default::default(),
        tile_cols_log2: 0,
        tile_rows_log2: 0,
    }
}

// ---- OBU framing ----

#[test]
fn obu_framing() {
    assert_eq!(obu(ObuType::TemporalDelimiter, &[]), [0x12, 0x00]);
    assert_eq!(temporal_delimiter(), [0x12, 0x00]);
    assert_eq!(obu(ObuType::SequenceHeader, &[1, 2, 3]), [0x0A, 3, 1, 2, 3]);
    assert_eq!(obu(ObuType::Frame, &[9])[0], 0x32);
    assert_eq!(obu(ObuType::FrameHeader, &[])[0], 0x1A);
    assert_eq!(obu(ObuType::TileGroup, &[])[0], 0x22);
    assert_eq!(obu(ObuType::Padding, &[])[0], 0x7A);
    let big = obu(ObuType::Padding, &vec![0; 300]);
    assert_eq!(&big[..3], [0x7A, 0xAC, 0x02]);
    assert_eq!(big.len(), 303);
}

// ---- sequence header against the decoder's parser ----

fn check_seq(s: &SeqParams) {
    let payload = write_sequence_header(s);
    let p = SequenceHeader::parse(&payload)
        .unwrap_or_else(|e| panic!("{}x{}: {e:?}", s.width, s.height));
    assert_eq!(p.profile, s.profile);
    assert!(!p.still_picture && !p.reduced_still_picture_header && !p.timing_info_present);
    assert_eq!(p.operating_points_cnt_minus_1, 0);
    assert_eq!(p.operating_point_idc[0], 0);
    assert_eq!(p.seq_level_idx[0], s.level_idx);
    assert_eq!((p.max_frame_width, p.max_frame_height), (s.width, s.height));
    assert!(!p.frame_id_numbers_present);
    assert_eq!(p.use_128x128_superblock, s.use_128x128_superblock);
    assert!(!p.enable_filter_intra && !p.enable_intra_edge_filter);
    assert!(
        !p.enable_interintra_compound
            && !p.enable_masked_compound
            && !p.enable_warped_motion
            && !p.enable_dual_filter
    );
    assert_eq!(p.enable_order_hint, s.enable_order_hint);
    assert!(!p.enable_jnt_comp && !p.enable_ref_frame_mvs);
    assert_eq!(p.seq_force_screen_content_tools, 0);
    assert_eq!(
        p.order_hint_bits,
        if s.enable_order_hint {
            ORDER_HINT_BITS
        } else {
            0
        }
    );
    assert!(!p.enable_superres);
    assert_eq!(p.enable_cdef, s.enable_cdef);
    assert_eq!(p.enable_restoration, s.enable_restoration);
    assert_eq!(p.color.bit_depth, s.bit_depth);
    assert!(!p.color.mono_chrome);
    assert_eq!((p.color.subsampling_x, p.color.subsampling_y), (1, 1));
    assert_eq!(p.color.color_range, s.full_range);
    assert_eq!(
        (
            p.color.color_primaries,
            p.color.transfer_characteristics,
            p.color.matrix_coefficients
        ),
        (
            s.color_primaries,
            s.transfer_characteristics,
            s.matrix_coefficients
        )
    );
    assert!(!p.color.separate_uv_delta_q);
    assert!(!p.film_grain_params_present);
}

#[test]
fn sequence_header_round_trips_through_decoder_parser() {
    let sizes = [
        (1, 1),
        (2, 2),
        (16, 16),
        (64, 64),
        (66, 66),
        (255, 256),
        (640, 360),
        (1920, 1080),
        (1921, 1081),
        (3840, 2160),
        (4096, 2304),
        (65536, 65536),
        (7, 65000),
    ];
    for &(w, h) in &sizes {
        for bits in [8u8, 10] {
            for flags in 0..32u32 {
                let mut s = seq(w, h);
                s.bit_depth = bits;
                s.use_128x128_superblock = flags & 1 != 0;
                s.enable_cdef = flags & 2 != 0;
                s.enable_restoration = flags & 4 != 0;
                s.enable_order_hint = flags & 8 != 0;
                s.full_range = flags & 16 != 0;
                s.level_idx = [0, 4, 8, 13, 31][(flags % 5) as usize];
                check_seq(&s);
            }
        }
    }
}

#[test]
fn sequence_header_colour_descriptions() {
    for (cp, tc, mc) in [
        (2, 2, 2),
        (1, 1, 1),
        (9, 16, 9),
        (1, 13, 1),
        (5, 6, 6),
        (255, 255, 255),
        (0, 0, 0),
    ] {
        let mut s = seq(320, 240);
        s.color_primaries = cp;
        s.transfer_characteristics = tc;
        s.matrix_coefficients = mc;
        check_seq(&s);
    }
}

#[test]
fn sequence_header_ends_with_trailing_bits() {
    let b = write_sequence_header(&seq(1920, 1080));
    assert!(
        *b.last().unwrap() != 0,
        "last byte carries the trailing one bit"
    );
}

#[test]
#[should_panic]
fn sequence_header_rejects_srgb_identity_triple_in_profile_0() {
    let mut s = seq(64, 64);
    (
        s.color_primaries,
        s.transfer_characteristics,
        s.matrix_coefficients,
    ) = (1, 13, 0);
    write_sequence_header(&s);
}

// ---- av1C ----

#[test]
fn av1c_layout() {
    let mut s = seq(1920, 1080);
    s.level_idx = 8;
    let c = av1c(&s);
    assert_eq!(c[0], 0x81);
    assert_eq!(c[1], 0x08); // profile 0, level idx 8
    assert_eq!(c[2], 0b0000_1100); // 8-bit, 4:2:0, csp 0
    assert_eq!(c[3], 0);
    let cfg = &c[4..];
    assert_eq!(cfg[0], 0x0A);
    assert_eq!(cfg[1] as usize, cfg.len() - 2);
    assert_eq!(&cfg[2..], &write_sequence_header(&s)[..]);

    s.bit_depth = 10;
    s.level_idx = 5;
    let c = av1c(&s);
    assert_eq!(c[1], 0x05);
    assert_eq!(c[2], 0b0100_1100);
    SequenceHeader::parse(&c[6..]).unwrap();
}

// ---- frame header ----

#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    frame_type: u32,
    show_frame: bool,
    showable: bool,
    error_resilient: bool,
    disable_cdf_update: bool,
    order_hint: u32,
    refresh: u32,
    disable_frame_end_update_cdf: bool,
    tile_cols_log2: u32,
    tile_rows_log2: u32,
    tile_size_bytes: u32,
    base_q_idx: u32,
    lf_levels: Vec<u32>,
    tx_mode_select: bool,
    reduced_tx_set: bool,
    bits: usize,
}

fn tile_log2(blk: u32, target: u32) -> u32 {
    (0..).find(|k| (blk << k) >= target).unwrap()
}

/// uncompressed_header() for an intra frame, read per spec 5.9.2 with the sequence's flags.
fn parse_frame_header(s: &SeqParams, data: &[u8]) -> Parsed {
    let mut r = Reader::new(data);
    assert_eq!(r.f(1), 0, "show_existing_frame");
    let frame_type = r.f(2);
    let show_frame = r.f(1) == 1;
    let showable = if show_frame {
        frame_type != 0
    } else {
        r.f(1) == 1
    };
    let error_resilient = if frame_type == 0 && show_frame {
        true
    } else {
        r.f(1) == 1
    };
    let disable_cdf_update = r.f(1) == 1;
    assert_eq!(r.f(1), 0, "frame_size_override_flag");
    let order_hint = if s.enable_order_hint {
        r.f(ORDER_HINT_BITS)
    } else {
        0
    };
    let refresh = if frame_type == 0 && show_frame {
        0xFF
    } else {
        r.f(8)
    };
    if !error_resilient && refresh != 0xFF {} // ref_order_hint only with error_resilient_mode
    assert_eq!(r.f(1), 0, "render_and_frame_size_different");
    let disable_frame_end_update_cdf = if disable_cdf_update {
        true
    } else {
        r.f(1) == 1
    };
    // tile_info()
    let mi_cols = 2 * ((s.width + 7) >> 3);
    let mi_rows = 2 * ((s.height + 7) >> 3);
    let sb_shift = if s.use_128x128_superblock { 5 } else { 4 };
    let sb_cols = (mi_cols + (1 << sb_shift) - 1) >> sb_shift;
    let sb_rows = (mi_rows + (1 << sb_shift) - 1) >> sb_shift;
    let sb_size = sb_shift + 2;
    let max_w = 4096 >> sb_size;
    let max_area = (4096 * 2304) >> (2 * sb_size);
    let min_cols = tile_log2(max_w, sb_cols);
    let max_cols = tile_log2(1, sb_cols.min(64));
    let max_rows = tile_log2(1, sb_rows.min(64));
    let min_tiles = min_cols.max(tile_log2(max_area, sb_rows * sb_cols));
    assert_eq!(r.f(1), 1, "uniform_tile_spacing_flag");
    let mut tc = min_cols;
    while tc < max_cols && r.f(1) == 1 {
        tc += 1;
    }
    let mut tr = min_tiles.saturating_sub(tc);
    while tr < max_rows && r.f(1) == 1 {
        tr += 1;
    }
    let mut tile_size_bytes = 0;
    if tc > 0 || tr > 0 {
        assert_eq!(r.f(tr + tc), 0, "context_update_tile_id");
        tile_size_bytes = r.f(2) + 1;
    }
    let base_q_idx = r.f(8);
    assert_eq!(
        (r.f(1), r.f(1), r.f(1), r.f(1)),
        (0, 0, 0, 0),
        "delta q / qmatrix"
    );
    assert_eq!(r.f(1), 0, "segmentation_enabled");
    if base_q_idx > 0 {
        assert_eq!(r.f(1), 0, "delta_q_present");
    }
    let mut lf_levels = vec![];
    let (mut tx_mode_select, lossless) = (false, base_q_idx == 0);
    if !lossless {
        lf_levels.push(r.f(6));
        lf_levels.push(r.f(6));
        if lf_levels[0] != 0 || lf_levels[1] != 0 {
            lf_levels.push(r.f(6));
            lf_levels.push(r.f(6));
        }
        assert_eq!(r.f(3), 0, "sharpness");
        assert_eq!(r.f(1), 0, "loop_filter_delta_enabled");
        if s.enable_cdef {
            assert_eq!(
                r.f(2 + 2 + 4 + 2 + 4 + 2),
                0,
                "cdef: one zero-strength entry"
            );
        }
        if s.enable_restoration {
            assert_eq!(r.f(6), 0, "lr_type NONE x3");
        }
        tx_mode_select = r.f(1) == 1;
    }
    let reduced_tx_set = r.f(1) == 1;
    Parsed {
        frame_type,
        show_frame,
        showable,
        error_resilient,
        disable_cdf_update,
        order_hint,
        refresh,
        disable_frame_end_update_cdf,
        tile_cols_log2: tc,
        tile_rows_log2: tr,
        tile_size_bytes,
        base_q_idx,
        lf_levels,
        tx_mode_select,
        reduced_tx_set,
        bits: r.pos(),
    }
}

fn header_bytes(s: &SeqParams, f: &FrameParams) -> (Vec<u8>, usize) {
    let mut w = BitWriter::new();
    write_frame_header(s, f, &mut w);
    let n = w.bit_len();
    (w.finish(), n)
}

#[test]
fn key_frame_header_fields_read_back() {
    for enable_oh in [false, true] {
        for (cdef, lr) in [(false, false), (true, false), (false, true), (true, true)] {
            for q in [0u8, 1, 37, 100, 255] {
                for lf in [[0, 0], [5, 0], [0, 9], [63, 63]] {
                    for cdf_off in [false, true] {
                        let mut s = seq(1920, 1080);
                        s.enable_order_hint = enable_oh;
                        s.enable_cdef = cdef;
                        s.enable_restoration = lr;
                        let mut f = key(q);
                        f.loop_filter_level = lf;
                        f.disable_cdf_update = cdf_off;
                        f.order_hint = 5;
                        let (b, nbits) = header_bytes(&s, &f);
                        let p = parse_frame_header(&s, &b);
                        assert_eq!(p.bits, nbits, "parser consumed exactly what was written");
                        assert_eq!(
                            (p.frame_type, p.show_frame, p.showable, p.error_resilient),
                            (0, true, false, true)
                        );
                        assert_eq!(p.disable_cdf_update, cdf_off);
                        assert_eq!(p.order_hint, if enable_oh { 5 } else { 0 });
                        assert_eq!(p.refresh, 0xFF);
                        assert!(p.disable_frame_end_update_cdf);
                        assert_eq!(p.base_q_idx, q as u32);
                        if q == 0 {
                            assert!(p.lf_levels.is_empty());
                        } else {
                            assert_eq!(p.lf_levels[..2], [lf[0] as u32, lf[1] as u32]);
                            assert_eq!(p.lf_levels.len(), if lf == [0, 0] { 2 } else { 4 });
                            assert!(!p.tx_mode_select);
                        }
                        assert!(!p.reduced_tx_set);
                    }
                }
            }
        }
    }
}

#[test]
fn hidden_key_and_intra_only_headers() {
    let s = seq(640, 360);
    let mut f = key(80);
    f.show_frame = false;
    f.refresh_frame_flags = 0x05;
    f.order_hint = 3;
    let (b, n) = header_bytes(&s, &f);
    let p = parse_frame_header(&s, &b);
    assert_eq!(
        (
            p.frame_type,
            p.show_frame,
            p.showable,
            p.error_resilient,
            p.refresh,
            p.bits
        ),
        (0, false, false, false, 0x05, n)
    );

    f.frame_type = FrameType::IntraOnly;
    f.show_frame = true;
    f.refresh_frame_flags = 0x81;
    let (b, n) = header_bytes(&s, &f);
    let p = parse_frame_header(&s, &b);
    assert_eq!(
        (
            p.frame_type,
            p.show_frame,
            p.showable,
            p.error_resilient,
            p.refresh,
            p.bits
        ),
        (2, true, true, false, 0x81, n)
    );
    f.show_frame = false;
    let (b, n) = header_bytes(&s, &f);
    let p = parse_frame_header(&s, &b);
    assert_eq!(
        (p.frame_type, p.show_frame, p.showable, p.refresh, p.bits),
        (2, false, true, 0x81, n)
    );
}

#[test]
#[should_panic]
fn intra_only_cannot_refresh_all_slots() {
    let mut f = key(10);
    f.frame_type = FrameType::IntraOnly;
    header_bytes(&seq(64, 64), &f);
}

#[test]
fn tile_info_layouts() {
    for &(w, h, sb128) in &[
        (64, 64, false),
        (1920, 1080, false),
        (1920, 1080, true),
        (3840, 2160, false),
        (8192, 4352, false),
        (16384, 4096, true),
        (66, 66, false),
        (7680, 4320, true),
    ] {
        for (cl, rl) in [(0u8, 0u8), (1, 0), (0, 1), (2, 2), (3, 1), (6, 6)] {
            let mut s = seq(w, h);
            s.use_128x128_superblock = sb128;
            let mut f = key(60);
            f.tile_cols_log2 = cl;
            f.tile_rows_log2 = rl;
            let (ec, er) = clamp_tile_log2(&s, cl, rl);
            let (b, n) = header_bytes(&s, &f);
            let p = parse_frame_header(&s, &b);
            assert_eq!(
                (p.tile_cols_log2, p.tile_rows_log2),
                (ec as u32, er as u32),
                "{w}x{h} sb128={sb128} req {cl},{rl}"
            );
            assert_eq!(p.bits, n);
            assert_eq!(p.tile_size_bytes, if ec > 0 || er > 0 { 4 } else { 0 });
            assert_eq!(p.base_q_idx, 60);
        }
    }
    // A frame wider than 4096 luma samples needs at least two tile columns.
    let (c, _) = clamp_tile_log2(&seq(8192, 4352), 0, 0);
    assert!(c >= 1);
    // Tiny frames cannot be split.
    assert_eq!(clamp_tile_log2(&seq(64, 64), 3, 3), (0, 0));
}

// ---- acceptance by the real decoder ----

fn temporal_unit(s: &SeqParams, f: &FrameParams) -> Vec<u8> {
    let mut tu = temporal_delimiter();
    tu.extend(obu(ObuType::SequenceHeader, &write_sequence_header(s)));
    tu.extend(frame_header_obu(s, f));
    tu
}

#[test]
fn decoder_accepts_temporal_unit_and_reads_the_sequence_back() {
    for &(w, h) in &[
        (64, 64),
        (66, 66),
        (640, 360),
        (1920, 1080),
        (1921, 1081),
        (4096, 2304),
    ] {
        for q in [0u8, 30, 255] {
            for (cdef, lr, sb128) in [(false, false, false), (true, true, true)] {
                let mut s = seq(w, h);
                s.enable_cdef = cdef;
                s.enable_restoration = lr;
                s.use_128x128_superblock = sb128;
                let mut f = key(q);
                f.loop_filter_level = [4, 6];
                let mut d = Decoder::new();
                let out = d
                    .decode(&temporal_unit(&s, &f))
                    .unwrap_or_else(|e| panic!("{w}x{h} q{q}: {e:?}"));
                assert!(out.is_empty(), "no tile data, so no picture yet");
                let p = d.sequence_header().expect("sequence header parsed");
                assert_eq!((p.max_frame_width, p.max_frame_height), (w, h));
                assert_eq!(p.enable_cdef, cdef);
            }
        }
    }
}

#[test]
fn decoder_accepts_tiled_headers_and_hidden_frames() {
    let s = seq(3840, 2160);
    for (cl, rl) in [(1, 0), (0, 1), (2, 1)] {
        let mut f = key(50);
        f.tile_cols_log2 = cl;
        f.tile_rows_log2 = rl;
        Decoder::new().decode(&temporal_unit(&s, &f)).unwrap();
    }
    let mut f = key(50);
    f.show_frame = false;
    f.refresh_frame_flags = 0x3;
    Decoder::new().decode(&temporal_unit(&s, &f)).unwrap();
}

#[test]
fn decoder_rejects_a_frame_header_without_the_sequence_header() {
    let s = seq(64, 64);
    let mut tu = temporal_delimiter();
    tu.extend(frame_header_obu(&s, &key(10)));
    assert!(Decoder::new().decode(&tu).is_err());
}

#[test]
fn frame_header_obu_has_trailing_bits() {
    let s = seq(640, 360);
    let o = frame_header_obu(&s, &key(77));
    assert_eq!(o[0], 0x1A);
    let payload = &o[2..];
    assert_eq!(payload.len(), o[1] as usize);
    let (_, n) = header_bytes(&s, &key(77));
    // The trailing one bit sits right after the header bits.
    let bit = (payload[n / 8] >> (7 - n % 8)) & 1;
    assert_eq!(bit, 1);
    assert_eq!(payload.len(), n / 8 + 1);
}

#[test]
fn decoder_notices_a_damaged_frame_header() {
    // Sanity for the acceptance tests above: the decoder really parses the header.
    let s = seq(1920, 1080);
    let mut f = key(50);
    f.tile_cols_log2 = 2;
    let good = frame_header_obu(&s, &f);
    let payload = &good[2..];
    let truncated = obu(ObuType::FrameHeader, &payload[..3]);
    let mut tu = temporal_delimiter();
    tu.extend(obu(ObuType::SequenceHeader, &write_sequence_header(&s)));
    tu.extend(truncated);
    assert!(Decoder::new().decode(&tu).is_err());
}

// ---- inter frame headers ----

/// A key frame then inter frames, read back by the decoder's own header parser: every field the
/// writer fixes must come out as written, and the parser must consume exactly the bits written.
#[test]
fn inter_frame_header_reads_back_with_the_decoders_parser() {
    use studio_av1::header::HeaderTracker;
    for &(w, h, q) in &[(64u32, 64u32, 60u8), (130, 70, 1), (1920, 1080, 200)] {
        let s = seq(w, h);
        let parsed_seq = SequenceHeader::parse(&write_sequence_header(&s)).unwrap();
        let mut t = HeaderTracker::new();
        let mut kw = BitWriter::new();
        write_frame_header(&s, &key(q), &mut kw);
        let (k, _) = t.parse_frame_header(&parsed_seq, &kw.finish(), 0, 0).unwrap();
        assert_eq!(k.frame_type, 0);
        for (i, (refresh, hint, idx)) in [(1u8, 1u32, [0u8; 7]), (3, 2, [0, 1, 0, 1, 0, 1, 0]), (0x80, 3, [1; 7])].into_iter().enumerate() {
            let mut f = key(q);
            f.frame_type = FrameType::Inter;
            f.refresh_frame_flags = refresh;
            f.order_hint = hint;
            f.ref_frame_idx = idx;
            f.disable_cdf_update = i == 1;
            let mut bw = BitWriter::new();
            write_frame_header(&s, &f, &mut bw);
            let bits = bw.bit_len();
            let (fh, used) = t.parse_frame_header(&parsed_seq, &bw.finish(), 0, 0).unwrap_or_else(|e| panic!("{w}x{h} q{q} inter {i}: {e:?}"));
            assert_eq!(used, bits, "the parser reads exactly the bits written");
            assert_eq!((fh.frame_type, fh.frame_is_intra, fh.show_frame), (1, false, true));
            assert_eq!(fh.refresh_frame_flags, refresh as u32);
            assert_eq!(fh.order_hint, hint);
            assert_eq!(fh.ref_frame_idx, idx.map(|v| v as usize));
            assert_eq!(fh.primary_ref_frame, 7);
            assert_eq!(fh.disable_cdf_update, i == 1);
            assert_eq!(fh.quant.base_q_idx, q as u32);
            assert!(!fh.allow_high_precision_mv && !fh.is_motion_mode_switchable && !fh.use_ref_frame_mvs);
            assert_eq!(fh.interpolation_filter, 0, "EIGHTTAP");
            assert!(!fh.reference_select && !fh.skip_mode_present && !fh.allow_warped_motion);
            assert!(fh.gm_type[1..8].iter().all(|&g| g == 0), "no global motion");
        }
    }
}
