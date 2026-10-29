// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M5: tests for the `recon` module.
//!
//! The reference reconstructs the same decisions with `studio-av1`'s public `predict_intra`
//! and `inverse_transform_2d`, and models availability differently from the implementation: a
//! plain "already reconstructed" bitmap per plane and mode maps per chroma unit, instead of the
//! spec's `BlockDecoded` flag arrays and neighbour index arithmetic.

use studio_av1::frame::Plane;
use studio_av1::intra::{IntraParams, predict_intra};
use studio_av1::spec_tables::*;
use studio_av1::transform::inverse_transform_2d;
use studio_av1_enc::recon::{FrameBuf, IntraExtra, QuantInfo, RefStore};
use studio_av1_enc::types::{BlockDecision, CoefBlock, SbPos};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

fn find_tx(w: usize, h: usize) -> usize {
    (0..TX_WIDTH.len())
        .find(|&t| TX_WIDTH[t] as usize == w && TX_HEIGHT[t] as usize == h)
        .unwrap()
}

/// One block of the layout plus what `BlockDecision` does not carry.
struct Blk {
    d: BlockDecision,
    extra: IntraExtra,
}

/// Transform blocks of a plane of a block: `(x, y, tx_size)` in plane samples, raster order,
/// only those starting inside the coded area.
fn tx_blocks(
    d: &BlockDecision,
    plane: usize,
    mi_rows: usize,
    mi_cols: usize,
    lossless: bool,
) -> Vec<(usize, usize, usize)> {
    let (bw, bh) = (d.w4 as usize * 4, d.h4 as usize * 4);
    let (x0, y0, w, h, tx) = if plane == 0 {
        (
            d.mi_col as usize * 4,
            d.mi_row as usize * 4,
            bw,
            bh,
            d.tx_size,
        )
    } else {
        let (cw, ch) = ((bw / 2).max(4), (bh / 2).max(4));
        (
            (d.mi_col as usize >> 1) * 4,
            (d.mi_row as usize >> 1) * 4,
            cw,
            ch,
            find_tx(cw.min(32), ch.min(32)),
        )
    };
    let tx = if lossless { TX_4X4 } else { tx };
    let (tw, th) = (TX_WIDTH[tx] as usize, TX_HEIGHT[tx] as usize);
    let sub = (plane > 0) as usize;
    let (max_x, max_y) = ((mi_cols * 4) >> sub, (mi_rows * 4) >> sub);
    let mut v = Vec::new();
    for y in (0..h).step_by(th) {
        for x in (0..w).step_by(tw) {
            if x0 + x < max_x && y0 + y < max_y {
                v.push((x0 + x, y0 + y, tx));
            }
        }
    }
    v
}

fn has_chroma(d: &BlockDecision) -> bool {
    !((d.h4 == 1 && d.mi_row & 1 == 0) || (d.w4 == 1 && d.mi_col & 1 == 0))
}

fn random_coefs(rng: &mut Rng, tx: usize, plane: u8) -> CoefBlock {
    let (w, h) = (TX_WIDTH[tx] as usize, TX_HEIGHT[tx] as usize);
    let (tw, th) = (w.min(32), h.min(32));
    let mut levels = vec![0i32; tw * th];
    let eob = if rng.below(5) == 0 {
        0
    } else {
        1 + rng.below(6) as usize
    };
    for k in 0..eob {
        let (i, j) = (
            rng.below(th.min(8) as u32) as usize,
            rng.below(tw.min(8) as u32) as usize,
        );
        let mag = 1 + rng.below(if k == 0 { 40 } else { 6 }) as i32;
        levels[i * tw + j] = if rng.below(2) == 0 { mag } else { -mag };
    }
    let tx_type = if w.max(h) <= 16 {
        [
            DCT_DCT,
            ADST_DCT,
            DCT_ADST,
            ADST_ADST,
            FLIPADST_DCT,
            DCT_FLIPADST,
            FLIPADST_FLIPADST,
            IDTX,
        ][rng.below(8) as usize]
    } else {
        DCT_DCT
    };
    CoefBlock {
        plane,
        tx_size: tx,
        tx_type,
        levels,
        eob,
    }
}

type Shape = (usize, usize, usize, usize);

/// Random z-order partition of one 64x64 superblock into valid block sizes (square, HORZ, VERT).
fn partition(
    rng: &mut Rng,
    r: usize,
    c: usize,
    n4: usize,
    dims: (usize, usize),
    fixed: Option<usize>,
    out: &mut Vec<Shape>,
) {
    let (mi_rows, mi_cols) = dims;
    if r >= mi_rows || c >= mi_cols {
        return;
    }
    let half = n4 / 2;
    let whole = r + half < mi_rows && c + half < mi_cols;
    // 0 NONE, 1 HORZ, 2 VERT, 3 SPLIT
    let pick = if n4 == 1 {
        0
    } else if !whole {
        3
    } else if let Some(f) = fixed {
        if n4 == f { 0 } else { 3 }
    } else {
        rng.below(4)
    };
    match pick {
        0 => out.push((r, c, n4, n4)),
        1 => {
            out.push((r, c, n4, half));
            if r + half < mi_rows {
                out.push((r + half, c, n4, half));
            }
        }
        2 => {
            out.push((r, c, half, n4));
            if c + half < mi_cols {
                out.push((r, c + half, half, n4));
            }
        }
        _ => {
            for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
                partition(rng, r + dr, c + dc, half, dims, fixed, out);
            }
        }
    }
}

fn layout(
    rng: &mut Rng,
    width: usize,
    height: usize,
    fixed: Option<usize>,
    lossless: bool,
) -> Vec<Vec<Blk>> {
    let (mi_cols, mi_rows) = (2 * ((width + 7) >> 3), 2 * ((height + 7) >> 3));
    let modes = [
        DC_PRED,
        V_PRED,
        H_PRED,
        D45_PRED,
        D135_PRED,
        D113_PRED,
        D157_PRED,
        D203_PRED,
        D67_PRED,
        SMOOTH_PRED,
        SMOOTH_V_PRED,
        SMOOTH_H_PRED,
        PAETH_PRED,
    ];
    let mut sbs = Vec::new();
    for sr in 0..mi_rows.div_ceil(16) {
        for sc in 0..mi_cols.div_ceil(16) {
            let mut shapes = Vec::new();
            partition(
                rng,
                sr * 16,
                sc * 16,
                16,
                (mi_rows, mi_cols),
                fixed,
                &mut shapes,
            );
            let mut blocks = Vec::new();
            for (r, c, w4, h4) in shapes {
                let max_tx = find_tx((w4 * 4).min(64), (h4 * 4).min(64));
                // square sub-transforms exercise the raster order inside a block
                let tx_size = if rng.below(3) == 0 && w4.min(h4) >= 2 {
                    let s = ((w4.min(h4) * 4).min(64) >> rng.below(2)).max(4);
                    find_tx(s, s)
                } else {
                    max_tx
                };
                let y_mode = modes[rng.below(13) as usize];
                let uv_mode = modes[rng.below(13) as usize];
                let mut d = BlockDecision {
                    mi_row: r as u32,
                    mi_col: c as u32,
                    w4: w4 as u32,
                    h4: h4 as u32,
                    y_mode,
                    uv_mode,
                    tx_size,
                    skip: rng.below(4) == 0,
                    coefs: vec![],
                };
                let ad = |rng: &mut Rng, m: usize| {
                    if w4.min(h4) >= 2 && (V_PRED..=D67_PRED).contains(&m) {
                        rng.below(7) as i32 - 3
                    } else {
                        0
                    }
                };
                let extra = IntraExtra {
                    angle_delta_y: ad(rng, y_mode),
                    angle_delta_uv: ad(rng, uv_mode),
                };
                if !d.skip {
                    for plane in 0..(1 + 2 * has_chroma(&d) as usize) {
                        for (_, _, tx) in tx_blocks(&d, plane, mi_rows, mi_cols, lossless) {
                            let cb = random_coefs(rng, tx, plane as u8);
                            d.coefs.push(cb);
                        }
                    }
                }
                blocks.push(Blk { d, extra });
            }
            sbs.push(blocks);
        }
    }
    sbs
}

/// The reference reconstruction.
struct Reference {
    planes: [Plane; 3],
    done: [Vec<bool>; 3], // 4x4 units already reconstructed, inside the coded area
    unit_w: [usize; 3],
    ymode: Vec<usize>,  // per luma 4x4 unit
    uvmode: Vec<usize>, // per chroma 4x4 unit
    mi_rows: usize,
    mi_cols: usize,
    bd: u32,
    q: QuantInfo,
}

impl Reference {
    fn new(width: usize, height: usize, bd: u32, q: QuantInfo) -> Reference {
        let (mi_cols, mi_rows) = (2 * ((width + 7) >> 3), 2 * ((height + 7) >> 3));
        let aw = width.div_ceil(128) * 128 + 160;
        let ah = height.div_ceil(128) * 128 + 160;
        let uw = [mi_cols, mi_cols / 2, mi_cols / 2];
        let uh = [mi_rows, mi_rows / 2, mi_rows / 2];
        Reference {
            planes: [
                Plane::new(aw, ah),
                Plane::new(aw / 2, ah / 2),
                Plane::new(aw / 2, ah / 2),
            ],
            done: [
                vec![false; uw[0] * uh[0]],
                vec![false; uw[1] * uh[1]],
                vec![false; uw[2] * uh[2]],
            ],
            unit_w: uw,
            ymode: vec![DC_PRED; mi_rows * mi_cols],
            uvmode: vec![DC_PRED; (mi_rows / 2) * (mi_cols / 2)],
            mi_rows,
            mi_cols,
            bd,
            q,
        }
    }

    fn unit_rows(&self, plane: usize) -> usize {
        if plane == 0 {
            self.mi_rows
        } else {
            self.mi_rows / 2
        }
    }

    fn is_done(&self, plane: usize, ux: isize, uy: isize) -> bool {
        ux >= 0
            && uy >= 0
            && (ux as usize) < self.unit_w[plane]
            && (uy as usize) < self.unit_rows(plane)
            && self.done[plane][uy as usize * self.unit_w[plane] + ux as usize]
    }

    fn smooth(m: usize) -> bool {
        m == SMOOTH_PRED || m == SMOOTH_V_PRED || m == SMOOTH_H_PRED
    }

    fn block(&mut self, b: &Blk) {
        let d = &b.d;
        let (mi_row, mi_col) = (d.mi_row as usize, d.mi_col as usize);
        let lossless = self.q.lossless();
        let hc = has_chroma(d);
        for r in mi_row..(mi_row + d.h4 as usize).min(self.mi_rows) {
            for c in mi_col..(mi_col + d.w4 as usize).min(self.mi_cols) {
                self.ymode[r * self.mi_cols + c] = d.y_mode;
            }
        }
        if hc {
            let (cx, cy) = (mi_col >> 1, mi_row >> 1);
            for r in cy..(cy + (d.h4 as usize / 2).max(1)).min(self.mi_rows / 2) {
                for c in cx..(cx + (d.w4 as usize / 2).max(1)).min(self.mi_cols / 2) {
                    self.uvmode[r * (self.mi_cols / 2) + c] = d.uv_mode;
                }
            }
        }
        let mut coefs = d.coefs.iter();
        for plane in 0..(1 + 2 * hc as usize) {
            let sub = (plane > 0) as usize;
            let (ox, oy) = if plane == 0 {
                (mi_col * 4, mi_row * 4)
            } else {
                ((mi_col >> 1) * 4, (mi_row >> 1) * 4)
            };
            let mode = if plane == 0 { d.y_mode } else { d.uv_mode };
            let angle = if plane == 0 {
                b.extra.angle_delta_y
            } else {
                b.extra.angle_delta_uv
            };
            // filter type: does the block's above or left neighbour use a smooth mode
            let (ux, uy) = ((ox / 4) as isize, (oy / 4) as isize);
            let get = |x: isize, y: isize| {
                if plane == 0 {
                    self.ymode[y as usize * self.mi_cols + x as usize]
                } else {
                    self.uvmode[y as usize * (self.mi_cols / 2) + x as usize]
                }
            };
            let ft = (uy > 0 && Self::smooth(get(ux, uy - 1)))
                || (ux > 0 && Self::smooth(get(ux - 1, uy)));
            for (x, y, tx) in tx_blocks(d, plane, self.mi_rows, self.mi_cols, lossless) {
                let (w, h) = (TX_WIDTH[tx] as usize, TX_HEIGHT[tx] as usize);
                let p = IntraParams {
                    plane,
                    x,
                    y,
                    have_left: x > 0,
                    have_above: y > 0,
                    have_above_right: self.is_done(
                        plane,
                        ((x + w) / 4) as isize,
                        (y / 4) as isize - 1,
                    ),
                    have_below_left: self.is_done(
                        plane,
                        (x / 4) as isize - 1,
                        ((y + h) / 4) as isize,
                    ),
                    mode,
                    log2w: TX_WIDTH_LOG2[tx] as u32,
                    log2h: TX_HEIGHT_LOG2[tx] as u32,
                    max_x: ((self.mi_cols * 4) >> sub) as i32 - 1,
                    max_y: ((self.mi_rows * 4) >> sub) as i32 - 1,
                    bit_depth: self.bd,
                    angle_delta: angle,
                    use_filter_intra: false,
                    filter_intra_mode: 0,
                    enable_intra_edge_filter: true,
                    filter_type: (V_PRED..=D67_PRED).contains(&mode) && ft,
                };
                predict_intra(&mut self.planes[plane], &p);
                if !d.skip {
                    let c = coefs.next().expect("coef per transform block");
                    assert_eq!((c.plane as usize, c.tx_size), (plane, tx));
                    if c.eob > 0 {
                        self.residual(plane, x, y, tx, c);
                    }
                }
                for uy in y / 4..(y + h) / 4 {
                    for ux in x / 4..(x + w) / 4 {
                        if ux < self.unit_w[plane] && uy < self.unit_rows(plane) {
                            self.done[plane][uy * self.unit_w[plane] + ux] = true;
                        }
                    }
                }
            }
        }
        assert!(coefs.next().is_none());
    }

    fn residual(&mut self, plane: usize, x: usize, y: usize, tx: usize, c: &CoefBlock) {
        let (w, h) = (TX_WIDTH[tx] as usize, TX_HEIGHT[tx] as usize);
        let (tw, th) = (w.min(32), h.min(32));
        // spec 7.12.3: dqDenom is 2 for 16x32 / 32x16 up to 32x32 / 16x64 / 64x16, 4 from 32x64
        let denom = match w * h {
            n if n >= 2048 => 4,
            n if n >= 512 => 2,
            _ => 1,
        };
        let bdi = ((self.bd - 8) >> 1) as usize;
        let (dcd, acd) = match plane {
            0 => (self.q.delta_q_y_dc, 0),
            1 => (self.q.delta_q_u_dc, self.q.delta_q_u_ac),
            _ => (self.q.delta_q_v_dc, self.q.delta_q_v_ac),
        };
        let qi = self.q.base_q_idx as i32;
        let dcq = DC_QLOOKUP[bdi][(qi + dcd).clamp(0, 255) as usize] as i64;
        let acq = AC_QLOOKUP[bdi][(qi + acd).clamp(0, 255) as usize] as i64;
        let mut dq = vec![0i32; 64 * 64];
        for i in 0..th {
            for j in 0..tw {
                let v = c.levels[i * tw + j] as i64 * if i + j == 0 { dcq } else { acq };
                let m = (v.abs() & 0xFFFFFF) / denom;
                dq[i * 64 + j] = (if v < 0 { -m } else { m })
                    .clamp(-(1 << (7 + self.bd)), (1 << (7 + self.bd)) - 1)
                    as i32;
            }
        }
        let mut res = vec![0i32; w * h];
        inverse_transform_2d(&dq, &mut res, tx, c.tx_type, self.q.lossless(), self.bd);
        let fud = [FLIPADST_DCT, FLIPADST_ADST, V_FLIPADST, FLIPADST_FLIPADST].contains(&c.tx_type);
        let flr = [DCT_FLIPADST, ADST_FLIPADST, H_FLIPADST, FLIPADST_FLIPADST].contains(&c.tx_type);
        for i in 0..h {
            for j in 0..w {
                let (yy, xx) = (
                    if fud { h - 1 - i } else { i },
                    if flr { w - 1 - j } else { j },
                );
                let v = self.planes[plane].at(x + xx, y + yy) as i32 + res[i * w + j];
                self.planes[plane].set(x + xx, y + yy, v.clamp(0, (1 << self.bd) - 1) as u16);
            }
        }
    }
}

fn run(width: usize, height: usize, bd: u8, q: QuantInfo, fixed: Option<usize>, seed: u64) {
    let mut rng = Rng(seed);
    let sbs = layout(&mut rng, width, height, fixed, q.lossless());
    let mut fb = FrameBuf::new(width as u32, height as u32, bd);
    fb.quant = q;
    let mut rf = Reference::new(width, height, bd as u32, q);
    let sb_cols = (2 * ((width + 7) >> 3)).div_ceil(16);
    for (i, blocks) in sbs.iter().enumerate() {
        fb.begin_sb(SbPos {
            col: (i % sb_cols) as u32,
            row: (i / sb_cols) as u32,
        });
        for b in blocks {
            fb.reconstruct_block_ex(&b.d, b.extra);
            rf.block(b);
        }
    }
    for p in 0..3 {
        if let Some(i) =
            (0..fb.planes[p].data.len()).find(|&i| fb.planes[p].data[i] != rf.planes[p].data[i])
        {
            let (x, y) = (i % fb.planes[p].stride, i / fb.planes[p].stride);
            let sub = (p > 0) as usize;
            let blk = sbs.iter().flatten().find(|b| {
                let d = &b.d;
                let (bx, by) = (
                    (d.mi_col as usize * 4) >> sub,
                    (d.mi_row as usize * 4) >> sub,
                );
                x >= bx
                    && y >= by
                    && x < bx + ((d.w4 as usize * 4) >> sub).max(4)
                    && y < by + ((d.h4 as usize * 4) >> sub).max(4)
            });
            if let Some(b) = blk {
                eprintln!(
                    "first diff plane {p} at ({x},{y}): block r{} c{} {}x{} ymode {} uvmode {} tx {} skip {} extra {:?}",
                    b.d.mi_row,
                    b.d.mi_col,
                    b.d.w4,
                    b.d.h4,
                    b.d.y_mode,
                    b.d.uv_mode,
                    b.d.tx_size,
                    b.d.skip,
                    b.extra
                );
            } else {
                eprintln!("first diff plane {p} at ({x},{y}), no block");
            }
        }
        assert!(
            fb.planes[p].data == rf.planes[p].data,
            "plane {p} differs ({width}x{height} bd{bd} q{} seed {seed})",
            q.base_q_idx
        );
    }
    // the cropped picture has the Frame layout and the right sizes
    let f = fb.to_frame();
    assert_eq!(f.planes[0].len(), width * height);
    assert_eq!(f.planes[1].len(), width.div_ceil(2) * height.div_ceil(2));
    for y in 0..height {
        assert_eq!(
            &f.planes[0][y * width..(y + 1) * width],
            &rf.planes[0].row(y)[..width]
        );
    }
    if bd == 8 {
        assert!(f.planes.iter().all(|p| p.iter().all(|&s| s < 256)));
    }
}

fn q(base: u8) -> QuantInfo {
    QuantInfo {
        base_q_idx: base,
        ..Default::default()
    }
}

#[test]
fn uniform_block_sizes_match_reference_8bit() {
    for (i, n4) in [1usize, 2, 4, 8, 16].into_iter().enumerate() {
        run(64, 64, 8, q(100), Some(n4), 10 + i as u64);
    }
}

#[test]
fn random_partitions_match_reference_8bit() {
    for seed in 0..6u64 {
        run(64, 64, 8, q(60 + 30 * seed as u8), None, 100 + seed);
    }
}

#[test]
fn odd_frame_sizes_match_reference_8bit() {
    for (i, &(w, h)) in [
        (66, 66),
        (17, 33),
        (130, 75),
        (8, 8),
        (200, 36),
        (1, 1),
        (99, 7),
    ]
    .iter()
    .enumerate()
    {
        run(w, h, 8, q(90), None, 200 + i as u64);
        run(w, h, 8, q(180), Some(4), 300 + i as u64);
    }
}

#[test]
fn chroma_deltas_and_extreme_quantisers() {
    let qi = QuantInfo {
        base_q_idx: 40,
        delta_q_y_dc: -3,
        delta_q_u_dc: 5,
        delta_q_u_ac: -4,
        delta_q_v_dc: 2,
        delta_q_v_ac: 7,
    };
    run(72, 40, 8, qi, None, 400);
    run(72, 40, 8, q(255), None, 401);
    run(72, 40, 8, q(1), None, 402);
}

#[test]
fn lossless_matches_reference() {
    run(66, 34, 8, q(0), None, 500);
}

#[test]
fn ten_bit_matches_reference() {
    run(66, 50, 10, q(120), None, 600);
}

#[test]
fn skip_blocks_are_pure_prediction() {
    let mut fb = FrameBuf::new(16, 16, 8);
    fb.quant = q(100);
    fb.begin_sb(SbPos { col: 0, row: 0 });
    let d = BlockDecision {
        mi_row: 0,
        mi_col: 0,
        w4: 4,
        h4: 4,
        y_mode: DC_PRED,
        uv_mode: DC_PRED,
        tx_size: TX_16X16,
        skip: true,
        coefs: vec![],
    };
    fb.reconstruct_block(&d);
    // no neighbours: DC predicts 1 << (bd - 1)
    assert!(fb.planes[0].row(5)[..16].iter().all(|&s| s == 128));
    assert!(fb.planes[1].row(3)[..8].iter().all(|&s| s == 128));
}

#[test]
fn ref_store_refresh_sets_masked_slots() {
    let mut rs = RefStore::default();
    let f = std::sync::Arc::new(FrameBuf::new(16, 16, 8));
    rs.refresh(0b1000_0101, f.clone());
    let set: Vec<bool> = rs.slots.iter().map(|s| s.is_some()).collect();
    assert_eq!(set, [true, false, true, false, false, false, false, true]);
    assert!(std::sync::Arc::ptr_eq(rs.slots[7].as_ref().unwrap(), &f));
}
