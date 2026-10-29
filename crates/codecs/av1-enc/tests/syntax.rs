// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M4: tests for the `syntax` module.
//!
//! The gate is a round trip through the real decoder: random decisions (every partition type,
//! all intra modes with angle deltas, every intra transform size and type, skip blocks, sparse and
//! dense coefficient blocks, huge levels) are written with `TileWriter`, wrapped in a key frame
//! built with M1's headers, decoded by `studio_av1::Decoder`, and the picture is compared sample
//! for sample with what M5's `FrameBuf` reconstructs from the same decisions. A single wrong
//! context or symbol desynchronises the decoder and shows up as a pixel difference.
//!
//! Everything is bounded: small frames, a fixed number of seeds, and size asserts on the output.

use studio_av1::spec_tables::*;
use studio_av1::Decoder;
use studio_av1_enc::bits::BitWriter;
use studio_av1_enc::obu::{ObuType, obu, temporal_delimiter, write_frame_header, write_sequence_header};
use studio_av1_enc::recon::{FrameBuf, IntraExtra, QuantInfo};
use studio_av1_enc::syntax::TileWriter;
use studio_av1_enc::syntax::coef::{intra_tx_set, plane_tx_type, TxbParams};
use studio_av1_enc::syntax::ctx::{block_size_of, find_tx_size, plane_residual_size};
use studio_av1_enc::transform::quant::scan_for;
use studio_av1_enc::types::{BlockDecision, CoefBlock, FrameParams, FrameType, SbDecision, SbPos, SeqParams};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

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
        tile_cols_log2: 0,
        tile_rows_log2: 0,
    }
}

type Shape = (usize, usize, usize, usize);

/// A random partition of the `bsize` block at `(r, c)` into leaf blocks, in decode order, written
/// from the definitions of the partition types rather than from the writer.
fn partition(rng: &mut Rng, r: usize, c: usize, bsize: usize, dims: (usize, usize), fixed: Option<usize>, out: &mut Vec<Shape>) {
    let (mi_rows, mi_cols) = dims;
    if r >= mi_rows || c >= mi_cols {
        return;
    }
    let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
    let half = n4 / 2;
    let quarter = half / 2;
    let has_rows = r + half < mi_rows;
    let has_cols = c + half < mi_cols;
    let p = if bsize < BLOCK_8X8 {
        PARTITION_NONE
    } else if has_rows && has_cols {
        if let Some(f) = fixed {
            if n4 == f { PARTITION_NONE } else { PARTITION_SPLIT }
        } else if rng.below(3) == 0 {
            PARTITION_SPLIT
        } else if bsize == BLOCK_8X8 {
            rng.below(4) as usize
        } else {
            rng.below(10) as usize
        }
    } else if has_cols {
        if rng.below(2) == 0 { PARTITION_HORZ } else { PARTITION_SPLIT }
    } else if has_rows {
        if rng.below(2) == 0 { PARTITION_VERT } else { PARTITION_SPLIT }
    } else {
        PARTITION_SPLIT
    };
    let sub = PARTITION_SUBSIZE[p][bsize] as usize;
    let split = PARTITION_SUBSIZE[PARTITION_SPLIT][bsize] as usize;
    let dim = |s: usize| (NUM_4X4_BLOCKS_WIDE[s] as usize, NUM_4X4_BLOCKS_HIGH[s] as usize);
    let mut leaf = |rr: usize, cc: usize, s: usize| out.push((rr, cc, dim(s).0, dim(s).1));
    match p {
        PARTITION_NONE => leaf(r, c, sub),
        PARTITION_HORZ => {
            leaf(r, c, sub);
            if has_rows {
                leaf(r + half, c, sub);
            }
        }
        PARTITION_VERT => {
            leaf(r, c, sub);
            if has_cols {
                leaf(r, c + half, sub);
            }
        }
        PARTITION_SPLIT => {
            for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
                partition(rng, r + dr, c + dc, split, dims, fixed, out);
            }
        }
        PARTITION_HORZ_A => {
            leaf(r, c, split);
            leaf(r, c + half, split);
            leaf(r + half, c, sub);
        }
        PARTITION_HORZ_B => {
            leaf(r, c, sub);
            leaf(r + half, c, split);
            leaf(r + half, c + half, split);
        }
        PARTITION_VERT_A => {
            leaf(r, c, split);
            leaf(r + half, c, split);
            leaf(r, c + half, sub);
        }
        PARTITION_VERT_B => {
            leaf(r, c, sub);
            leaf(r, c + half, split);
            leaf(r + half, c + half, split);
        }
        PARTITION_HORZ_4 => {
            for k in 0..4 {
                if k < 3 || r + quarter * k < mi_rows {
                    leaf(r + quarter * k, c, sub);
                }
            }
        }
        _ => {
            for k in 0..4 {
                if k < 3 || c + quarter * k < mi_cols {
                    leaf(r, c + quarter * k, sub);
                }
            }
        }
    }
}

/// Transform blocks of a plane of a block that start inside the coded area: `tx_size` in decode
/// order (raster inside the block).
fn tx_blocks(d: &BlockDecision, plane: usize, mi_rows: usize, mi_cols: usize, lossless: bool) -> Vec<usize> {
    let (bw, bh) = (d.w4 as usize * 4, d.h4 as usize * 4);
    let (x0, y0, w, h, tx) = if plane == 0 {
        (d.mi_col as usize * 4, d.mi_row as usize * 4, bw, bh, d.tx_size)
    } else {
        let (cw, ch) = ((bw / 2).max(4), (bh / 2).max(4));
        ((d.mi_col as usize >> 1) * 4, (d.mi_row as usize >> 1) * 4, cw, ch, find_tx_size(cw.min(32), ch.min(32)))
    };
    let tx = if lossless { TX_4X4 } else { tx };
    let (tw, th) = (TX_WIDTH[tx] as usize, TX_HEIGHT[tx] as usize);
    let sub = (plane > 0) as usize;
    let (max_x, max_y) = ((mi_cols * 4) >> sub, (mi_rows * 4) >> sub);
    let mut v = Vec::new();
    for y in (0..h).step_by(th) {
        for x in (0..w).step_by(tw) {
            if x0 + x < max_x && y0 + y < max_y {
                v.push(tx);
            }
        }
    }
    v
}

fn has_chroma(d: &BlockDecision) -> bool {
    !((d.h4 == 1 && d.mi_row & 1 == 0) || (d.w4 == 1 && d.mi_col & 1 == 0))
}

/// How the levels of a random block are shaped.
#[derive(Clone, Copy)]
enum Density {
    Sparse,
    Mixed,
    Full,
}

fn random_coefs(rng: &mut Rng, d: &BlockDecision, tx: usize, plane: usize, q: u8, big: bool) -> CoefBlock {
    let (w, h) = (TX_WIDTH[tx] as usize, TX_HEIGHT[tx] as usize);
    let (tw, th) = (w.min(32), h.min(32));
    let n = tw * th;
    let p = TxbParams { plane, x4: 0, y4: 0, mi_size: 0, y_mode: d.y_mode, uv_mode: d.uv_mode, lossless: q == 0, qidx_nonzero: q > 0 };
    let luma_type = if plane == 0 && q > 0 {
        let set = intra_tx_set(tx);
        let allowed: Vec<usize> = (0..16).filter(|&t| set == 0 && t == DCT_DCT || set > 0 && TX_TYPE_IN_SET_INTRA[set][t] == 1).collect();
        allowed[rng.below(allowed.len() as u32) as usize]
    } else {
        DCT_DCT
    };
    let tx_type = plane_tx_type(&p, tx, luma_type);
    let scan = scan_for(tx, tx_type);
    let mut levels = vec![0i32; n];
    let density = match rng.below(6) {
        0 | 1 => Density::Sparse,
        2 | 3 | 4 => Density::Mixed,
        _ => Density::Full,
    };
    let eob = if rng.below(5) == 0 {
        0
    } else {
        match density {
            Density::Sparse => 1 + rng.below(6) as usize,
            Density::Mixed => 1 + rng.below(n.min(80) as u32) as usize,
            Density::Full => 1 + rng.below(n as u32) as usize,
        }
    };
    let mag = |rng: &mut Rng| -> i32 {
        let m = match rng.below(20) {
            0 => 1 + rng.below(3000) as i32, // Golomb tails of several lengths
            1 if big => 0xFFFFF - rng.below(3) as i32,
            2..=5 => 1 + rng.below(16) as i32, // around the base range
            _ => 1 + rng.below(3) as i32,
        };
        if rng.below(2) == 0 { m } else { -m }
    };
    for c in 0..eob {
        let dense = match density {
            Density::Sparse => rng.below(2) == 0,
            Density::Mixed => rng.below(3) != 0,
            Density::Full => true,
        };
        if c == eob - 1 || dense {
            levels[scan[c] as usize] = mag(rng);
        }
    }
    CoefBlock { plane: plane as u8, tx_size: tx, tx_type, levels, eob }
}

struct Case {
    sbs: Vec<(SbPos, SbDecision, Vec<IntraExtra>)>,
    mi_rows: usize,
    mi_cols: usize,
}

/// Random decisions for a frame of `width` x `height`. `select` picks transform sizes below the
/// largest (TX_MODE_SELECT); `fixed` forces uniform blocks of that many 4x4 units.
fn gen_case(rng: &mut Rng, width: usize, height: usize, q: u8, select: bool, fixed: Option<usize>, big: bool) -> Case {
    let (mi_cols, mi_rows) = (2 * ((width + 7) >> 3), 2 * ((height + 7) >> 3));
    let lossless = q == 0;
    let mut sbs = Vec::new();
    for sr in 0..mi_rows.div_ceil(16) {
        for sc in 0..mi_cols.div_ceil(16) {
            let mut shapes = Vec::new();
            partition(rng, sr * 16, sc * 16, BLOCK_64X64, (mi_rows, mi_cols), fixed, &mut shapes);
            let mut blocks = Vec::new();
            let mut extras = Vec::new();
            for (r, c, w4, h4) in shapes {
                let ms = block_size_of(w4, h4).unwrap();
                let mut tx_size = MAX_TX_SIZE_RECT[ms] as usize;
                if lossless {
                    tx_size = TX_4X4;
                } else if select && ms > BLOCK_4X4 {
                    for _ in 0..rng.below(3) {
                        if tx_size != TX_4X4 {
                            tx_size = SPLIT_TX_SIZE[tx_size] as usize;
                        }
                    }
                }
                let y_mode = rng.below(13) as usize;
                let uv_mode = rng.below(13) as usize;
                let mut d = BlockDecision { mi_row: r as u32, mi_col: c as u32, w4: w4 as u32, h4: h4 as u32, y_mode, uv_mode, tx_size, skip: rng.below(4) == 0, coefs: vec![] };
                let ad = |rng: &mut Rng, m: usize| if ms >= BLOCK_8X8 && (V_PRED..=D67_PRED).contains(&m) { rng.below(7) as i32 - 3 } else { 0 };
                let extra = IntraExtra { angle_delta_y: ad(rng, y_mode), angle_delta_uv: if has_chroma(&d) { ad(rng, uv_mode) } else { 0 } };
                if !d.skip {
                    for plane in 0..(1 + 2 * has_chroma(&d) as usize) {
                        for tx in tx_blocks(&d, plane, mi_rows, mi_cols, lossless) {
                            let cb = random_coefs(rng, &d, tx, plane, q, big);
                            d.coefs.push(cb);
                        }
                    }
                }
                blocks.push(d);
                extras.push(extra);
            }
            sbs.push((SbPos { col: sc as u32, row: sr as u32 }, SbDecision { blocks }, extras));
        }
    }
    Case { sbs, mi_rows, mi_cols }
}

struct Encoded {
    tile: Vec<u8>,
    recon: FrameBuf,
}

fn encode(s: &SeqParams, case: &Case, q: u8, select: bool, disable_update: bool) -> Encoded {
    let mut tw = TileWriter::new(s, q, case.mi_cols as u32, case.mi_rows as u32).with_tx_mode_select(select).with_disable_cdf_update(disable_update);
    let mut fb = FrameBuf::new(s.width, s.height, 8);
    fb.quant = QuantInfo { base_q_idx: q, ..Default::default() };
    fb.enable_intra_edge_filter = false; // the sequence header M1 writes turns it off
    for (pos, sb, extras) in &case.sbs {
        tw.write_sb_ex(*pos, sb, extras);
        fb.begin_sb(*pos);
        for (b, e) in sb.blocks.iter().zip(extras) {
            fb.reconstruct_block_ex(b, *e);
        }
    }
    Encoded { tile: tw.finish(), recon: fb }
}

/// A temporal unit: delimiter, sequence header, and a frame OBU (header + tile data). `select`
/// flips the header's `tx_mode_select` bit, which M1's writer leaves at zero.
fn stream(s: &SeqParams, f: &FrameParams, tile: &[u8], select: bool) -> Vec<u8> {
    let mut w = BitWriter::new();
    write_frame_header(s, f, &mut w);
    let n = w.bit_len();
    let mut hdr = w.as_bytes().to_vec();
    if select {
        assert!(f.base_q_idx > 0);
        // the last two header bits are tx_mode_select and reduced_tx_set
        let bit = n - 2;
        assert_eq!(hdr[bit / 8] >> (7 - bit % 8) & 1, 0, "tx_mode_select is not where expected");
        hdr[bit / 8] |= 0x80 >> (bit % 8);
    }
    hdr.extend_from_slice(tile);
    let mut tu = temporal_delimiter();
    tu.extend(obu(ObuType::SequenceHeader, &write_sequence_header(s)));
    tu.extend(obu(ObuType::Frame, &hdr));
    tu
}

fn decode(tu: &[u8]) -> Result<studio_av1::Picture, String> {
    let mut d = Decoder::new();
    let mut pics = d.decode(tu).map_err(|e| format!("{e:?}"))?;
    pics.extend(d.flush_result().map_err(|e| format!("{e:?}"))?);
    pics.pop().ok_or_else(|| "no picture".to_string())
}

fn check(width: usize, height: usize, q: u8, select: bool, fixed: Option<usize>, seed: u64, disable_update: bool) {
    let mut rng = Rng(seed);
    let s = seq(width as u32, height as u32);
    let case = gen_case(&mut rng, width, height, q, select, fixed, seed % 3 == 0);
    let enc = encode(&s, &case, q, select, disable_update);
    // Bounded output: at most ~8 bytes per sample even for dense random levels.
    assert!(enc.tile.len() < 8 * width * height + 4096, "tile of {} bytes is out of bounds", enc.tile.len());
    let mut f = key(q);
    f.disable_cdf_update = disable_update;
    let pic = decode(&stream(&s, &f, &enc.tile, select)).unwrap_or_else(|e| panic!("{width}x{height} q{q} seed {seed}: decode failed: {e}"));
    let want = enc.recon.to_frame();
    assert_eq!((pic.width as usize, pic.height as usize), (width, height));
    for p in 0..3 {
        assert_eq!(pic.planes[p].len(), want.planes[p].len(), "plane {p} size");
        if let Some(i) = (0..pic.planes[p].len()).find(|&i| pic.planes[p][i] != want.planes[p][i]) {
            let pw = if p == 0 { width } else { width.div_ceil(2) };
            panic!("{width}x{height} q{q} select {select} seed {seed}: plane {p} first differs at ({}, {}): decoded {} expected {}", i % pw, i / pw, pic.planes[p][i], want.planes[p][i]);
        }
    }
}

#[test]
fn uniform_block_sizes() {
    for (i, n4) in [1usize, 2, 4, 8, 16].into_iter().enumerate() {
        check(64, 64, 100, false, Some(n4), 10 + i as u64, false);
    }
}

#[test]
fn random_partitions_largest_transform() {
    for seed in 0..12 {
        check(128, 96, [1u8, 40, 120, 255][seed as usize % 4], false, None, 100 + seed, false);
    }
}

#[test]
fn random_partitions_select_transform_size() {
    for seed in 0..12 {
        check(128, 96, [5u8, 60, 200, 255][seed as usize % 4], true, None, 200 + seed, false);
    }
}

#[test]
fn odd_frame_sizes() {
    for (i, &(w, h)) in [(1usize, 1usize), (4, 4), (8, 8), (9, 7), (66, 66), (130, 34), (34, 130), (72, 40), (200, 18)].iter().enumerate() {
        check(w, h, 90, false, None, 300 + i as u64, false);
        check(w, h, 90, true, None, 350 + i as u64, false);
    }
}

#[test]
fn lossless_frames() {
    check(66, 34, 0, false, None, 500, false);
    check(64, 64, 0, false, Some(16), 501, false);
    check(8, 8, 0, false, None, 502, false);
}

#[test]
fn cdf_adaptation_off() {
    for seed in 0..4 {
        check(96, 64, 80, seed % 2 == 0, None, 600 + seed, true);
    }
}

#[test]
fn many_seeds_small_frames() {
    for seed in 0..40 {
        let (w, h) = (16 + 8 * (seed as usize % 7), 16 + 8 * (seed as usize % 5));
        check(w, h, [1u8, 33, 90, 180, 255][seed as usize % 5], seed % 2 == 1, None, 700 + seed, false);
    }
}

#[test]
fn the_decoder_notices_damage() {
    // Sanity for the round trip: the comparison is sensitive to a flipped bit in the tile data.
    let (w, h, q) = (64usize, 64usize, 60u8);
    let mut rng = Rng(900);
    let s = seq(w as u32, h as u32);
    let case = gen_case(&mut rng, w, h, q, false, None, false);
    let enc = encode(&s, &case, q, false, false);
    let want = enc.recon.to_frame();
    let mut bad = enc.tile.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0x10;
    match decode(&stream(&s, &key(q), &bad, false)) {
        Err(_) => {}
        Ok(pic) => assert!(pic.planes.iter().zip(&want.planes).any(|(a, b)| a != b), "a damaged tile decoded to the same picture"),
    }
}

#[test]
fn several_superblock_rows_and_columns() {
    // Four superblocks of uniform 64x64 blocks: the left context is cleared per row, the above
    // context and the adaptation carry across superblocks.
    check(128, 128, 70, false, Some(16), 950, false);
}

#[test]
#[should_panic(expected = "eob must end on a nonzero level")]
fn inconsistent_eob_is_rejected() {
    let s = seq(8, 8);
    let mut tw = TileWriter::new(&s, 50, 2, 2);
    let mut levels = vec![0i32; 64];
    levels[0] = 1;
    let cb = CoefBlock { plane: 0, tx_size: TX_8X8, tx_type: DCT_DCT, levels, eob: 5 };
    let d = BlockDecision { mi_row: 0, mi_col: 0, w4: 2, h4: 2, y_mode: DC_PRED, uv_mode: DC_PRED, tx_size: TX_8X8, skip: false, coefs: vec![cb.clone(), CoefBlock { plane: 1, tx_size: TX_4X4, levels: vec![0; 16], eob: 0, ..cb.clone() }, CoefBlock { plane: 2, tx_size: TX_4X4, levels: vec![0; 16], eob: 0, ..cb }] };
    tw.write_block(&d);
}

#[test]
#[should_panic(expected = "must split")]
fn a_block_past_both_edges_must_split() {
    let s = seq(8, 8);
    let mut tw = TileWriter::new(&s, 50, 2, 2);
    // an 8x8 frame is 2x2 mi, so a 16x16 block at the origin has no rows or columns to spare
    tw.write_partition(0, 0, BLOCK_16X16, PARTITION_NONE);
}

#[test]
fn plane_sizes_helper_matches_the_decoder_tables() {
    assert_eq!(plane_residual_size(BLOCK_64X64, 1), BLOCK_32X32);
    assert_eq!(plane_residual_size(BLOCK_64X64, 0), BLOCK_64X64);
    assert_eq!(plane_residual_size(BLOCK_4X4, 1), BLOCK_4X4);
}
