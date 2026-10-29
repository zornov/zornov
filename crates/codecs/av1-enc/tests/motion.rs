// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M7: tests for inter frames: the inter syntax writers, inter reconstruction and the motion
//! vector candidates.
//!
//! The gate is a round trip through the real decoder. A key frame comes from the `Encoder`; then
//! random inter decisions for a second frame (every partition type that keeps blocks at 8x8 or
//! more, all four modes, all seven reference names, `ref_mv_idx` up to what the stack reaches,
//! skip blocks, dense and sparse coefficients, inter transform types) are written with
//! `TileWriter`, reconstructed with `FrameBuf::reconstruct_inter_block`, and decoded by
//! `studio_av1::Decoder`. The picture must equal the reconstruction sample for sample. A wrong
//! context, candidate or prediction desynchronises the decoder or shows as a pixel difference.

use std::sync::Arc;

use studio_av1::Decoder;
use studio_av1::spec_tables::*;
use studio_av1_enc::bits::BitWriter;
use studio_av1_enc::motion::mvpred::{find_stack, frame_info, predicted_mv};
use studio_av1_enc::obu::{ObuType, obu, write_frame_header};
use studio_av1_enc::recon::{FrameBuf, QuantInfo, RefStore};
use studio_av1_enc::syntax::TileWriter;
use studio_av1_enc::syntax::coef::{inter_tx_set, intra_tx_set, plane_tx_type, TxbParams};
use studio_av1_enc::syntax::ctx::{block_size_of, find_tx_size, ContextState};
use studio_av1_enc::transform::quant::scan_for;
use studio_av1_enc::types::{BlockDecision, BlockMode, CoefBlock, Frame, FrameParams, FrameType, InterDecision, InterSbDecision, SbPos, SeqParams};
use studio_av1_enc::{Config, Encoder, RateMode};

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

fn picture(w: usize, h: usize, seed: u64) -> Frame {
    let mut rng = Rng(seed);
    let mut f = Frame::new(w as u32, h as u32, 8);
    for p in 0..3 {
        let (pw, ph) = if p == 0 { (w, h) } else { (w.div_ceil(2), h.div_ceil(2)) };
        for y in 0..ph {
            for x in 0..pw {
                let base = (x * 3 + y * 2 + p * 40) % 200 + 20;
                f.planes[p][y * f.stride[p] + x] = (base as u32 + rng.below(12)) as u16;
            }
        }
    }
    f
}

fn to_framebuf(f: &Frame) -> FrameBuf {
    let mut fb = FrameBuf::new(f.width, f.height, f.bit_depth);
    for p in 0..3 {
        let (pw, ph) = if p == 0 { (f.width as usize, f.height as usize) } else { ((f.width as usize).div_ceil(2), (f.height as usize).div_ceil(2)) };
        for y in 0..ph {
            for x in 0..pw {
                fb.planes[p].set(x, y, f.planes[p][y * f.stride[p] + x]);
            }
        }
    }
    fb
}

type Shape = (usize, usize, usize, usize);

/// A random partition into blocks of at least 8x8 (no 4-way split of 16x16, no split of 8x8).
fn partition(rng: &mut Rng, split_in_4: u32, r: usize, c: usize, bsize: usize, dims: (usize, usize), out: &mut Vec<Shape>) {
    let (mi_rows, mi_cols) = dims;
    if r >= mi_rows || c >= mi_cols {
        return;
    }
    let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
    let half = n4 / 2;
    let quarter = half / 2;
    let has_rows = r + half < mi_rows;
    let has_cols = c + half < mi_cols;
    let p = if bsize == BLOCK_8X8 {
        PARTITION_NONE
    } else if has_rows && has_cols {
        let max = if bsize == BLOCK_16X16 { 8 } else { 10 };
        if rng.below(4) < split_in_4 { PARTITION_SPLIT } else { rng.below(max) as usize }
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
                partition(rng, split_in_4, r + dr, c + dc, split, dims, out);
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

fn random_coefs(rng: &mut Rng, tx: usize, plane: usize, tx_type: usize) -> CoefBlock {
    let (w, h) = (TX_WIDTH[tx] as usize, TX_HEIGHT[tx] as usize);
    let n = w.min(32) * h.min(32);
    let scan = scan_for(tx, tx_type);
    let mut levels = vec![0i32; n];
    let eob = if rng.below(5) == 0 {
        0
    } else {
        match rng.below(3) {
            0 => 1 + rng.below(5) as usize,
            1 => 1 + rng.below(n.min(60) as u32) as usize,
            _ => 1 + rng.below(n as u32) as usize,
        }
    };
    for c in 0..eob {
        if c == eob - 1 || rng.below(2) == 0 {
            let m = match rng.below(12) {
                0 => 1 + rng.below(2000) as i32,
                1..=3 => 1 + rng.below(16) as i32,
                _ => 1 + rng.below(3) as i32,
            };
            levels[scan[c] as usize] = if rng.below(2) == 0 { m } else { -m };
        }
    }
    CoefBlock { plane: plane as u8, tx_size: tx, tx_type, levels, eob }
}

/// What the generator saw, to check the test exercises the syntax.
#[derive(Default, Debug)]
struct Stats {
    modes: [usize; 4],
    refs: [usize; 8],
    drl: [usize; 4],
    new_nonzero: usize,
    big_diffs: usize,
    skips: usize,
    blocks: usize,
    intra: usize,
}

struct Gen {
    /// Out of 10, how many blocks are intra.
    intra_in_10: u32,
    rng: Rng,
    /// Mirror of the mode info the writer keeps, for the candidate stacks.
    ctx: ContextState,
    mv_frame: studio_av1::mvpred::MvFrame,
    q: u8,
    stats: Stats,
}

impl Gen {
    /// A random block: inter, or intra with probability `intra_in_10 / 10`.
    fn any_block(&mut self, shape: Shape) -> BlockMode {
        if self.rng.below(10) < self.intra_in_10 {
            BlockMode::Intra(self.intra_block(shape))
        } else {
            BlockMode::Inter(self.block(shape))
        }
    }

    fn intra_block(&mut self, (r, c, w4, h4): Shape) -> BlockDecision {
        let ms = block_size_of(w4, h4).unwrap();
        let y_mode = self.rng.below(13) as usize;
        let uv_mode = self.rng.below(13) as usize;
        let skip = self.rng.below(4) == 0;
        let tx_luma = MAX_TX_SIZE_RECT[ms] as usize;
        let (cw, ch) = ((w4 * 4) / 2, (h4 * 4) / 2);
        let tx_chroma = find_tx_size(cw.min(32), ch.min(32));
        let mut coefs = Vec::new();
        if !skip {
            let set = intra_tx_set(tx_luma);
            let allowed: Vec<usize> = (0..16).filter(|&t| if set == 0 { t == DCT_DCT } else { TX_TYPE_IN_SET_INTRA[set][t] == 1 }).collect();
            let luma_type = allowed[self.rng.below(allowed.len() as u32) as usize];
            coefs.push(random_coefs(&mut self.rng, tx_luma, 0, luma_type));
            let p = TxbParams { plane: 1, x4: 0, y4: 0, mi_size: ms, y_mode, uv_mode, lossless: false, qidx_nonzero: self.q > 0, is_inter: false, luma_tx_type: DCT_DCT };
            let chroma_type = plane_tx_type(&p, tx_chroma, DCT_DCT);
            for plane in 1..3 {
                coefs.push(random_coefs(&mut self.rng, tx_chroma, plane, chroma_type));
            }
        }
        self.ctx.store_block(r, c, ms, y_mode, skip, tx_luma);
        self.stats.intra += 1;
        self.stats.blocks += 1;
        BlockDecision { mi_row: r as u32, mi_col: c as u32, w4: w4 as u32, h4: h4 as u32, y_mode, uv_mode, tx_size: tx_luma, skip, coefs }
    }

    fn block(&mut self, (r, c, w4, h4): Shape) -> InterDecision {
        let ms = block_size_of(w4, h4).unwrap();
        // mostly LAST, so that neighbours match and the stacks fill up
        let ref_frame = if self.rng.below(10) < 7 { 1 } else { 1 + self.rng.below(7) as u8 };
        let stack = find_stack(&self.ctx.mi, &self.mv_frame, (r, c, w4, h4), ref_frame);
        let n = stack.num_mv_found;
        let mode = [NEARESTMV, NEARMV, GLOBALMV, NEWMV][self.rng.below(4) as usize];
        let ref_mv_idx = match mode {
            NEARMV => 1 + self.rng.below(if n >= 4 { 3 } else if n >= 3 { 2 } else { 1 }) as usize,
            NEWMV => self.rng.below(if n > 2 { 3 } else if n > 1 { 2 } else { 1 }) as usize,
            _ => 0,
        };
        let pred = predicted_mv(&stack, mode, ref_mv_idx);
        let mv = if mode == NEWMV {
            let comp = |rng: &mut Rng, p: i32| -> i32 {
                match rng.below(4) {
                    0 => p,
                    1 => p + 2 * (rng.below(9) as i32 - 4) ,
                    2 => p + 2 * (rng.below(80) as i32 - 40),
                    _ => p + 2 * (rng.below(1500) as i32 - 750),
                }
            };
            let m = [comp(&mut self.rng, pred[0]), comp(&mut self.rng, pred[1])];
            // keep the vector inside what the syntax and the bitstream allow
            [m[0].clamp(-8000, 8000) & !1, m[1].clamp(-8000, 8000) & !1]
        } else {
            pred
        };
        // the difference to the prediction must be even in both components
        let mv = if mode == NEWMV { [mv[0] + (pred[0] & 1), mv[1] + (pred[1] & 1)] } else { mv };
        assert!(mode != NEWMV || ((mv[0] - pred[0]) % 2 == 0 && (mv[1] - pred[1]) % 2 == 0));
        let skip = self.rng.below(4) == 0;
        let tx_luma = MAX_TX_SIZE_RECT[ms] as usize;
        let (cw, ch) = ((w4 * 4) / 2, (h4 * 4) / 2);
        let tx_chroma = find_tx_size(cw.min(32), ch.min(32));
        let mut coefs = Vec::new();
        if !skip {
            let set = inter_tx_set(tx_luma);
            let allowed: Vec<usize> = (0..16).filter(|&t| if set == 0 { t == DCT_DCT } else { TX_TYPE_IN_SET_INTER[set][t] == 1 }).collect();
            let luma_type = allowed[self.rng.below(allowed.len() as u32) as usize];
            let luma = random_coefs(&mut self.rng, tx_luma, 0, luma_type);
            let eff = if luma.eob > 0 { luma.tx_type } else { DCT_DCT };
            let p = TxbParams { plane: 1, x4: 0, y4: 0, mi_size: ms, y_mode: DC_PRED, uv_mode: DC_PRED, lossless: false, qidx_nonzero: self.q > 0, is_inter: true, luma_tx_type: eff };
            let chroma_type = plane_tx_type(&p, tx_chroma, eff);
            coefs.push(luma);
            for plane in 1..3 {
                coefs.push(random_coefs(&mut self.rng, tx_chroma, plane, chroma_type));
            }
        }
        self.ctx.store_inter_block(r, c, ms, mode, skip, tx_luma, ref_frame as i8, mv);
        let s = &mut self.stats;
        s.blocks += 1;
        s.modes[[NEARESTMV, NEARMV, GLOBALMV, NEWMV].iter().position(|&m| m == mode).unwrap()] += 1;
        s.refs[ref_frame as usize] += 1;
        s.drl[ref_mv_idx] += 1;
        s.skips += skip as usize;
        if mode == NEWMV && mv != pred {
            s.new_nonzero += 1;
            if (mv[0] - pred[0]).abs().max((mv[1] - pred[1]).abs()) > 128 {
                s.big_diffs += 1;
            }
        }
        InterDecision { mi_row: r as u32, mi_col: c as u32, w4: w4 as u32, h4: h4 as u32, mode, ref_frame, ref_mv_idx, mv, skip, coefs }
    }
}

/// Encodes a key frame with the facade and a second, inter frame with random decisions, decodes
/// both, and checks the second picture against the reconstruction. Returns the stats.
fn round_trip(w: usize, h: usize, q: u8, seed: u64, split_in_4: u32, intra_in_10: u32) -> Stats {
    let key_src = picture(w, h, seed);
    let mut enc = Encoder::new(Config { rate: RateMode::Cqp(q), speed: 9, ..Config::new(w as u32, h as u32) }).unwrap();
    let key_pkt = enc.push_frame(&key_src, 0).unwrap().remove(0);
    let key_recon = enc.reconstruction().unwrap().clone();

    let s = seq(w as u32, h as u32);
    let (mi_cols, mi_rows) = (2 * ((w + 7) >> 3), 2 * ((h + 7) >> 3));
    let mut g = Gen { intra_in_10, rng: Rng(seed ^ 0x5eed), ctx: ContextState::new(mi_cols, mi_rows), mv_frame: frame_info(mi_rows, mi_cols), q, stats: Stats::default() };
    let mut refs = RefStore::default();
    refs.refresh(1, Arc::new(to_framebuf(&key_recon)));
    let ref_idx = [0u8; 7];
    let mut recon = FrameBuf::new(w as u32, h as u32, 8);
    recon.quant = QuantInfo { base_q_idx: q, ..Default::default() };
    recon.enable_intra_edge_filter = false;
    let mut tw = TileWriter::new(&s, q, mi_cols as u32, mi_rows as u32);
    for sr in 0..mi_rows.div_ceil(16) {
        for sc in 0..mi_cols.div_ceil(16) {
            let mut shapes = Vec::new();
            partition(&mut g.rng, split_in_4, sr * 16, sc * 16, BLOCK_64X64, (mi_rows, mi_cols), &mut shapes);
            let mut sb = InterSbDecision::default();
            recon.begin_sb(SbPos { col: sc as u32, row: sr as u32 });
            for shape in shapes {
                let b = g.any_block(shape);
                match &b {
                    BlockMode::Inter(d) => recon.reconstruct_inter_block(&refs, &ref_idx, d),
                    BlockMode::Intra(d) => recon.reconstruct_block(d),
                }
                sb.blocks.push(b);
            }
            tw.write_inter_sb(SbPos { col: sc as u32, row: sr as u32 }, &sb);
        }
    }
    let tile = tw.finish();
    assert!(tile.len() < 12 * w * h + 4096, "tile of {} bytes is out of bounds", tile.len());

    let fp = FrameParams {
        frame_type: FrameType::Inter,
        show_frame: true,
        base_q_idx: q,
        disable_cdf_update: false,
        refresh_frame_flags: 1,
        order_hint: 1,
        ref_frame_idx: ref_idx,
        loop_filter_level: [0, 0],
        cdef_enabled: false,
        tile_cols_log2: 0,
        tile_rows_log2: 0,
    };
    let mut bw = BitWriter::new();
    write_frame_header(&s, &fp, &mut bw);
    bw.byte_align();
    bw.put_bytes(&tile);
    let inter_tu = obu(ObuType::Frame, &bw.finish());

    let mut dec = Decoder::new();
    let mut pics = dec.decode(&key_pkt.data).unwrap();
    pics.extend(dec.decode(&inter_tu).unwrap_or_else(|e| panic!("{w}x{h} q{q} seed {seed}: inter frame: {e:?}")));
    pics.extend(dec.flush_result().unwrap());
    assert_eq!(pics.len(), 2, "{w}x{h} q{q} seed {seed}");
    assert_eq!(pics[0].planes, key_recon.planes, "the key frame");
    let want = recon.to_frame();
    for p in 0..3 {
        if let Some(i) = (0..want.planes[p].len()).find(|&i| pics[1].planes[p][i] != want.planes[p][i]) {
            let pw = if p == 0 { w } else { w.div_ceil(2) };
            panic!("{w}x{h} q{q} seed {seed}: plane {p} differs first at ({}, {}): decoded {} expected {}", i % pw, i / pw, pics[1].planes[p][i], want.planes[p][i]);
        }
    }
    g.stats
}

#[test]
fn inter_frames_decode_bit_exactly() {
    let mut total = Stats::default();
    for (i, &(w, h)) in [(64usize, 64usize), (66, 66), (130, 70), (200, 120), (34, 130), (16, 16)].iter().enumerate() {
        for (j, &q) in [20u8, 100, 220].iter().enumerate() {
            let s = round_trip(w, h, q, 1000 + (i * 10 + j) as u64, 1 + (i + j) as u32 % 3, 0);
            for k in 0..4 {
                total.modes[k] += s.modes[k];
                total.drl[k] += s.drl[k];
            }
            for k in 0..8 {
                total.refs[k] += s.refs[k];
            }
            total.new_nonzero += s.new_nonzero;
            total.big_diffs += s.big_diffs;
            total.skips += s.skips;
            total.blocks += s.blocks;
        }
    }
    eprintln!("{total:?}");
    assert!(total.modes.iter().all(|&n| n > 20), "every mode is exercised: {:?}", total.modes);
    assert!(total.refs[1..].iter().all(|&n| n > 5), "every reference name is exercised: {:?}", total.refs);
    assert!(total.drl[1] > 10 && total.drl[2] > 3, "DRL indices up to 2: {:?}", total.drl);
    assert!(total.new_nonzero > 50 && total.big_diffs > 10, "NEWMV differences of several classes");
}

#[test]
fn fine_partitions_fill_the_candidate_stacks() {
    // many small blocks with mostly one reference give stacks of up to four candidates
    let mut drl = [0usize; 4];
    for seed in 0..10u64 {
        let s = round_trip(128, 96, 60, 9000 + seed, 3, 0);
        for k in 0..4 {
            drl[k] += s.drl[k];
        }
    }
    eprintln!("ref_mv_idx counts {drl:?}");
    assert!(drl[1] > 20 && drl[2] > 5 && drl[3] > 0, "stacks reach four candidates: {drl:?}");
}

#[test]
fn many_seeds_small_frames() {
    for seed in 0..30u64 {
        let (w, h) = (8 + 8 * (seed as usize % 9), 8 + 8 * (seed as usize % 7));
        round_trip(w, h, [1u8, 40, 130, 255][seed as usize % 4], 5000 + seed, 1 + seed as u32 % 3, 0);
    }
}

/// Intra blocks among inter blocks: the intra mode info of inter frames, intra prediction from
/// inter neighbours (availability of above-right and below-left), and the stacks that skip intra
/// neighbours.
#[test]
fn intra_blocks_inside_inter_frames_decode_bit_exactly() {
    let mut intra = 0;
    for (i, &(w, h)) in [(64usize, 64usize), (66, 66), (130, 70), (200, 120), (24, 40)].iter().enumerate() {
        for (j, &q) in [20u8, 100, 230].iter().enumerate() {
            let s = round_trip(w, h, q, 7000 + (i * 10 + j) as u64, 1 + (i + j) as u32 % 3, [2, 5, 8][(i + j) % 3]);
            intra += s.intra;
        }
    }
    eprintln!("{intra} intra blocks among the inter blocks");
    assert!(intra > 100);
}

#[test]
fn candidate_stack_follows_the_neighbours() {
    // a block next to an inter block with a known vector sees it as the nearest candidate
    let (mi_cols, mi_rows) = (16, 16);
    let mut ctx = ContextState::new(mi_cols, mi_rows);
    let f = frame_info(mi_rows, mi_cols);
    ctx.store_inter_block(0, 0, BLOCK_32X32, NEWMV, false, TX_32X32, 1, [-24, 16]);
    let st = find_stack(&ctx.mi, &f, (0, 8, 8, 8), 1);
    assert!(st.num_mv_found >= 1);
    assert_eq!(st.ref_stack_mv[0][0], [-24, 16]);
    assert_eq!(predicted_mv(&st, NEARESTMV, 0), [-24, 16]);
    assert_eq!(predicted_mv(&st, GLOBALMV, 0), [0, 0]);
    // another reference frame finds no match: the stack falls back to the global vector
    let other = find_stack(&ctx.mi, &f, (0, 8, 8, 8), 2);
    assert_eq!(predicted_mv(&other, GLOBALMV, 0), [0, 0]);
}

#[test]
#[should_panic(expected = "derives")]
fn a_vector_that_is_not_the_modes_is_rejected() {
    let s = seq(64, 64);
    let mut tw = TileWriter::new(&s, 60, 16, 16);
    let d = InterDecision { mi_row: 0, mi_col: 0, w4: 16, h4: 16, mode: NEARESTMV, ref_frame: 1, ref_mv_idx: 0, mv: [8, 8], skip: true, coefs: vec![] };
    tw.write_inter_block(&d);
}

#[test]
#[should_panic(expected = "even")]
fn an_odd_new_vector_difference_is_rejected() {
    let s = seq(64, 64);
    let mut tw = TileWriter::new(&s, 60, 16, 16);
    let d = InterDecision { mi_row: 0, mi_col: 0, w4: 16, h4: 16, mode: NEWMV, ref_frame: 1, ref_mv_idx: 0, mv: [3, 0], skip: true, coefs: vec![] };
    tw.write_inter_block(&d);
}

/// The inter coder takes intra blocks where prediction from the reference fails: content that
/// appears from nowhere (a new bright square) is coded intra, the static rest inter.
#[test]
fn the_decider_codes_new_content_as_intra_blocks() {
    use studio_av1_enc::motion::InterFrameCoder;
    let (w, h) = (128usize, 128usize);
    let a = picture(w, h, 1);
    let mut b = a.clone();
    // a bright, flat square in the second frame only, away from anything in the first
    for y in 40..72 {
        for x in 40..72 {
            b.planes[0][y * b.stride[0] + x] = 235;
        }
    }
    for p in 1..3 {
        for y in 20..36 {
            for x in 20..36 {
                b.planes[p][y * b.stride[p] + x] = 100 + 30 * p as u16;
            }
        }
    }
    let mut enc = Encoder::new(Config { rate: RateMode::Cqp(60), speed: 9, ..Config::new(w as u32, h as u32) }).unwrap();
    enc.push_frame(&a, 0).unwrap();
    let mut refs = RefStore::default();
    refs.refresh(1, Arc::new(to_framebuf(enc.reconstruction().unwrap())));
    let mut recon = FrameBuf::new(w as u32, h as u32, 8);
    recon.quant = QuantInfo { base_q_idx: 60, ..Default::default() };
    recon.enable_intra_edge_filter = false;
    let mut coder = InterFrameCoder::new(&b, &refs, [0; 7], 4, true);
    let (mut intra, mut inter) = (0, 0);
    for sr in 0..2 {
        for sc in 0..2 {
            // 16x16 blocks, so that the flat square has interior blocks
            for blk in coder.decide_sb(&mut recon, SbPos { col: sc, row: sr }).blocks {
                match blk {
                    BlockMode::Intra(_) => intra += 1,
                    BlockMode::Inter(_) => inter += 1,
                }
            }
        }
    }
    eprintln!("{intra} intra and {inter} inter blocks");
    assert!(intra > 0 && inter > 0, "both kinds are chosen ({intra} intra, {inter} inter)");
}
