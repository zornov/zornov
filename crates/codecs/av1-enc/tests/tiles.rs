// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M10: tiles. The layout follows spec 5.9.15, tiled streams decode bit-exactly with `studio-av1`
//! and on the video hardware, and the bytes do not depend on the thread count.
//!
//! The streams here are built from the `tiles` module's pieces directly, with every tile coded as
//! an independent picture of the tile's size: for intra coding a tile only sees its own samples
//! and mode info (the decoder treats a tile edge like a frame edge), so that is what the decoder
//! reconstructs. This checks the layout, the frame header's tile info, the tile group framing and
//! the parallel fan-out without the encoder's own frame loop.

use studio_av1::Decoder;
use studio_av1_enc::color::from_rgba8;
use studio_av1_enc::mode::{FixedDecider, ModeDecider, PartitionDecider};
use studio_av1_enc::obu::{ObuType, obu, write_frame_header, write_sequence_header};
use studio_av1_enc::recon::{FrameBuf, QuantInfo};
use studio_av1_enc::syntax::TileWriter;
use studio_av1_enc::tiles::{TileLayout, encode_tiles, tile_group};
use studio_av1_enc::types::{FrameParams, FrameType, SbPos, SeqParams};
use studio_av1_enc::Frame;

fn noise(x: u32, y: u32) -> i32 {
    let mut v = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca6b);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2c1b_3c6d);
    v ^= v >> 12;
    (v % 11) as i32 - 5
}

fn picture(w: u32, h: u32) -> Frame {
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let edge = if (x / 17 + y / 29) % 2 == 0 { 50 } else { 0 };
            [((x / 3 + y / 5) as i32 % 256 + noise(x, y)).clamp(0, 255) as u8, ((y / 2 % 200 + edge) as i32 + noise(y, x)).clamp(0, 255) as u8, ((x * y / 900 % 256) as i32 + noise(x + 7, y)).clamp(0, 255) as u8, 255]
        })
        .collect();
    from_rgba8(&rgba, w, h, 8, false)
}

fn seq_for(w: u32, h: u32) -> SeqParams {
    let area = w as u64 * h as u64;
    SeqParams { width: w, height: h, bit_depth: 8, fps: (30, 1), profile: 0, level_idx: if area <= 2_359_296 { 8 } else { 12 }, use_128x128_superblock: false, enable_cdef: true, enable_restoration: true, enable_order_hint: true, color_primaries: 1, transfer_characteristics: 1, matrix_coefficients: 1, full_range: false }
}

/// The part of `src` at luma `(x0..x1, y0..y1)` (`x0`, `y0` even) as a picture of its own.
fn crop(src: &Frame, x0: u32, x1: u32, y0: u32, y1: u32) -> Frame {
    let mut f = Frame::new(x1 - x0, y1 - y0, 8);
    for p in 0..3 {
        let s = if p == 0 { 0 } else { 1 };
        let (px0, px1, py0, py1) = (x0 as usize >> s, (x1 as usize + s) >> s, y0 as usize >> s, (y1 as usize + s) >> s);
        for y in py0..py1 {
            let (a, b) = (y * src.stride[p] + px0, y * src.stride[p] + px1);
            f.planes[p][(y - py0) * f.stride[p]..(y - py0) * f.stride[p] + (px1 - px0)].copy_from_slice(&src.planes[p][a..b]);
        }
    }
    f
}

fn paste(dst: &mut Frame, tile: &Frame, x0: u32, y0: u32) {
    for p in 0..3 {
        let s = if p == 0 { 0 } else { 1 };
        let (px0, py0) = (x0 as usize >> s, y0 as usize >> s);
        let (tw, th) = (((tile.width as usize) + s) >> s, ((tile.height as usize) + s) >> s);
        for y in 0..th {
            let d = (py0 + y) * dst.stride[p] + px0;
            dst.planes[p][d..d + tw].copy_from_slice(&tile.planes[p][y * tile.stride[p]..y * tile.stride[p] + tw]);
        }
    }
}

struct Tiled {
    /// Sequence header OBU and frame OBU, as one MP4 sample.
    data: Vec<u8>,
    seq_obu_len: usize,
    recon: Frame,
    tiles: usize,
}

/// Codes `src` as a key frame split in `2^cols_log2` x `2^rows_log2` tiles on `threads` workers.
fn encode_intra_tiled(src: &Frame, cols_log2: u8, rows_log2: u8, q: u8, fixed: bool, threads: usize) -> Tiled {
    let seq = seq_for(src.width, src.height);
    let (layout, cl, rl) = TileLayout::for_frame(&seq, cols_log2, rows_log2);
    let coded: Vec<(Vec<u8>, Frame)> = encode_tiles(&layout, threads, |_, rect| {
        let (x0, x1, y0, y1) = rect.luma(64, src.width, src.height);
        let piece = crop(src, x0, x1, y0, y1);
        let (w, h) = (piece.width, piece.height);
        let (mi_cols, mi_rows) = (2 * ((w + 7) >> 3), 2 * ((h + 7) >> 3));
        let mut buf = FrameBuf::new(w, h, 8);
        buf.quant = QuantInfo { base_q_idx: q, ..Default::default() };
        buf.enable_intra_edge_filter = false;
        let mut tw = TileWriter::new(&seq_for(w, h), q, mi_cols, mi_rows);
        let mut decider: Box<dyn ModeDecider> = if fixed { Box::new(FixedDecider { base_q_idx: q }) } else { Box::new(PartitionDecider { base_q_idx: q, min_n4: 2 }) };
        for row in 0..mi_rows.div_ceil(16) {
            for col in 0..mi_cols.div_ceil(16) {
                let pos = SbPos { col, row };
                let sb = decider.decide_sb(&piece, &mut buf, pos);
                tw.write_sb(pos, &sb);
            }
        }
        (tw.finish(), buf.to_frame())
    });
    let mut recon = Frame::new(src.width, src.height, 8);
    for (i, (_, tile)) in coded.iter().enumerate() {
        let (x0, _, y0, _) = layout.rect(i).luma(64, src.width, src.height);
        paste(&mut recon, tile, x0, y0);
    }
    let fp = FrameParams {
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
        tile_cols_log2: cl,
        tile_rows_log2: rl,
    };
    let mut w = studio_av1_enc::bits::BitWriter::new();
    write_frame_header(&seq, &fp, &mut w);
    w.byte_align();
    let bytes: Vec<Vec<u8>> = coded.into_iter().map(|c| c.0).collect();
    w.put_bytes(&tile_group(&bytes));
    let mut data = obu(ObuType::SequenceHeader, &write_sequence_header(&seq));
    let seq_obu_len = data.len();
    data.extend(obu(ObuType::Frame, &w.finish()));
    Tiled { data, seq_obu_len, recon, tiles: layout.count() }
}

fn decode(data: &[u8]) -> Frame {
    let mut dec = Decoder::new();
    let mut out = dec.decode_pts(data, 0).expect("the decoder accepts the tiled frame");
    out.extend(dec.flush());
    assert_eq!(out.len(), 1);
    let p = &out[0];
    let mut f = Frame::new(p.width as u32, p.height as u32, 8);
    f.planes = p.planes.clone();
    f
}

#[test]
fn uniform_layout_follows_the_spec() {
    // 1080p with 64x64 superblocks: 30 x 17
    let l = TileLayout::uniform(30, 17, 0, 0);
    assert_eq!((l.col_starts.clone(), l.row_starts.clone()), (vec![0, 30], vec![0, 17]));
    let l = TileLayout::uniform(30, 17, 1, 0);
    assert_eq!(l.col_starts, [0, 15, 30]);
    let l = TileLayout::uniform(30, 17, 2, 1);
    assert_eq!(l.col_starts, [0, 8, 16, 24, 30]); // 4 tiles of 8, the last of 6
    assert_eq!(l.row_starts, [0, 9, 17]);
    assert_eq!(l.count(), 8);
    // too few superblocks to fill the requested count: fewer tiles than 1 << log2
    let l = TileLayout::uniform(5, 3, 3, 2);
    assert_eq!(l.col_starts, [0, 1, 2, 3, 4, 5]);
    assert_eq!(l.row_starts, [0, 1, 2, 3]);
    let l = TileLayout::uniform(5, 3, 2, 0);
    assert_eq!(l.col_starts, [0, 2, 4, 5]);
    // every layout covers the frame exactly, in order
    for (sc, sr) in [(1u32, 1u32), (2, 7), (30, 17), (60, 34), (64, 64)] {
        for cl in 0..=6u8 {
            for rl in 0..=6u8 {
                let l = TileLayout::uniform(sc, sr, cl, rl);
                for (starts, n, log2) in [(&l.col_starts, sc, cl), (&l.row_starts, sr, rl)] {
                    assert_eq!((starts[0], *starts.last().unwrap()), (0, n));
                    assert!(starts.windows(2).all(|w| w[0] < w[1]));
                    assert!(starts.len() - 1 <= 1 << log2);
                }
            }
        }
    }
    // the layout the header writes is the one the encoder codes
    let (l, c, r) = TileLayout::for_frame(&seq_for(1920, 1080), 2, 1);
    assert_eq!((c, r, l.count()), (2, 1, 8));
    let (l, c, r) = TileLayout::for_frame(&seq_for(64, 64), 3, 3);
    assert_eq!((c, r, l.count()), (0, 0, 1));
}

#[test]
fn tile_group_framing() {
    assert_eq!(tile_group(&[vec![1, 2, 3]]), [1, 2, 3]);
    assert_eq!(tile_group(&[vec![7, 8], vec![9]]), [0, 1, 0, 0, 0, 7, 8, 9]);
}

#[test]
fn tiled_intra_frames_decode_bit_exactly() {
    for &(w, h) in &[(330u32, 200u32), (193, 131), (256, 130), (65, 300)] {
        let src = picture(w, h);
        for (cl, rl) in [(0u8, 0u8), (1, 0), (0, 1), (1, 1), (2, 1)] {
            let t = encode_intra_tiled(&src, cl, rl, 100, false, 2);
            let got = decode(&t.data);
            assert_eq!(got.planes, t.recon.planes, "{w}x{h} tiles {cl},{rl} ({} tiles)", t.tiles);
        }
    }
}

#[test]
fn tiled_1080p_decodes_bit_exactly_and_is_the_same_at_any_thread_count() {
    let src = picture(1920, 1080);
    for (cl, rl) in [(1u8, 0u8), (1, 1), (2, 1)] {
        let one = encode_intra_tiled(&src, cl, rl, 120, true, 1);
        for threads in [3usize, 8] {
            let many = encode_intra_tiled(&src, cl, rl, 120, true, threads);
            assert_eq!(one.data, many.data, "tiles {cl},{rl}: bytes differ between 1 and {threads} threads");
        }
        assert_eq!(decode(&one.data).planes, one.recon.planes, "tiles {cl},{rl}");
    }
}

#[test]
fn tiled_streams_decode_on_videotoolbox_bit_exactly() {
    use studio_encode::mux::{Muxer, Options, av1c_from_sequence_header};
    if !studio_hw::Av1Decoder::supported() {
        eprintln!("SKIPPED: this machine's video hardware does not decode AV1");
        return;
    }
    let dir = std::env::temp_dir().join(format!("studio-av1-enc-tiles-vt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for &(w, h, cl, rl) in &[(1920u32, 1080u32, 1u8, 0u8), (1920, 1080, 2, 1), (330, 200, 1, 1), (193, 131, 1, 0)] {
        let src = picture(w, h);
        let t = encode_intra_tiled(&src, cl, rl, 110, w >= 1000, 4);
        let info = av1c_from_sequence_header(&t.data[..t.seq_obu_len]).unwrap();
        let path = dir.join(format!("tiles-{w}x{h}-{cl}{rl}.mp4"));
        let mut mux = Muxer::create(&path, Options::mp4_av1()).unwrap();
        let track = mux.add_av1_video(info.track_info(30, 1, 1_000_000)).unwrap();
        mux.write_temporal_unit(track, &t.data).unwrap();
        mux.finish().unwrap();
        let mut demux = studio_mp4::open(&path).unwrap();
        let movie = demux.movie().clone();
        let v = &movie.tracks[0];
        let studio_mp4::Codec::Av1 { config } = &v.codec else { panic!("not AV1: {:?}", v.codec) };
        let mut hw = studio_hw::Av1Decoder::new(&config.raw, w, h).expect("a hardware AV1 decoder");
        let mut got = Vec::new();
        for (i, s) in v.samples.iter().enumerate() {
            got.extend(hw.decode(&demux.read_sample(s).unwrap(), i as i64).unwrap());
        }
        got.extend(hw.flush());
        assert_eq!(got.len(), 1, "{w}x{h}");
        let d = &got[0];
        let exact = [(&d.y, &t.recon.planes[0]), (&d.u, &t.recon.planes[1]), (&d.v, &t.recon.planes[2])].iter().all(|(a, b)| a.len() == b.len() && a.iter().zip(b.iter()).all(|(&x, &y)| x as u16 == y));
        eprintln!("{w}x{h} tiles {cl},{rl} ({} tiles): VideoToolbox matches bit for bit: {exact}", t.tiles);
        assert!(exact, "{w}x{h} tiles {cl},{rl}: VideoToolbox output differs");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn merging_tile_buffers_copies_exactly_the_tile() {
    use studio_av1::frame::MiInfo;
    use studio_av1_enc::tiles::{merge_mode_info, merge_recon};
    let (w, h) = (330u32, 200u32);
    let (mi_cols, mi_rows) = (2 * ((w as usize + 7) >> 3), 2 * ((h as usize + 7) >> 3));
    let layout = TileLayout::uniform(6, 4, 2, 1); // 3 x 2 tiles of 2x2 superblocks (the last column has 1)
    assert_eq!(layout.count(), 6);
    // one source per tile, each filled with its own tile number
    let mut dst_mi = MiInfo::new(mi_cols, mi_rows, false);
    let mut dst = FrameBuf::new(w, h, 8);
    for t in 0..layout.count() {
        let mut mi = MiInfo::new(mi_cols, mi_rows, false);
        let mut buf = FrameBuf::new(w, h, 8);
        mi.y_mode.fill(t as u8 + 1);
        mi.mv.fill([studio_av1::frame::Mv::ZERO; 2]);
        mi.skip.fill(t % 2 == 0);
        for p in 0..3 {
            buf.planes[p].data.fill(t as u16 + 1);
        }
        merge_mode_info(&mut dst_mi, &mi, layout.rect(t), 64);
        merge_recon(&mut dst, &buf, layout.rect(t), 64);
    }
    for r in 0..mi_rows {
        for c in 0..mi_cols {
            let (tr, tc) = (layout.row_starts.iter().rposition(|&s| s as usize * 16 <= r).unwrap().min(layout.rows() - 1), layout.col_starts.iter().rposition(|&s| s as usize * 16 <= c).unwrap().min(layout.cols() - 1));
            let t = tr * layout.cols() + tc;
            assert_eq!(dst_mi.y_mode[dst_mi.idx(r, c)], t as u8 + 1, "mode info ({r},{c})");
            assert_eq!(dst_mi.skip[dst_mi.idx(r, c)], t % 2 == 0);
        }
    }
    for y in 0..h as usize {
        for x in 0..w as usize {
            let (tr, tc) = (layout.row_starts.iter().rposition(|&s| s as usize * 64 <= y).unwrap().min(layout.rows() - 1), layout.col_starts.iter().rposition(|&s| s as usize * 64 <= x).unwrap().min(layout.cols() - 1));
            assert_eq!(dst.planes[0].at(x, y), (tr * layout.cols() + tc) as u16 + 1, "luma ({x},{y})");
        }
    }
}

// ---- the encoder's own tiled frame loop ----

use studio_av1_enc::{Config, Encoder, RateMode};

/// A textured picture that pans by (3, 2) samples a frame.
fn pan_frame(w: u32, h: u32, i: u32) -> Frame {
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|j| {
            let (x, y) = (j % w + 3 * i, j / w + 2 * i);
            let edge = if (x / 24 + y / 40) % 2 == 0 { 50 } else { 0 };
            [((x / 3 + y / 5) as i32 % 256 + noise(x, y)).clamp(0, 255) as u8, ((y / 2 % 200 + edge) as i32 + noise(y, x)).clamp(0, 255) as u8, ((x * y / 900 % 256) as i32 + noise(x + 7, y)).clamp(0, 255) as u8, 255]
        })
        .collect();
    from_rgba8(&rgba, w, h, 8, false)
}

struct Coded {
    packets: Vec<Vec<u8>>,
    recon: Vec<Frame>,
}

fn encode_seq(w: u32, h: u32, frames: u32, tiles: (u8, u8), speed: u8, q: u8, threads: usize) -> Coded {
    let mut enc = Encoder::new(Config { speed, tiles, threads, rate: RateMode::Cqp(q), keyint: 240, ..Config::new(w, h) }).unwrap();
    let (mut packets, mut recon) = (Vec::new(), Vec::new());
    for i in 0..frames {
        packets.push(enc.push_frame(&pan_frame(w, h, i), i as i64).unwrap().remove(0).data);
        recon.push(enc.reconstruction().unwrap().clone());
    }
    Coded { packets, recon }
}

fn check_decodes(c: &Coded, what: &str) {
    let mut dec = Decoder::new();
    for (i, (p, want)) in c.packets.iter().zip(&c.recon).enumerate() {
        let mut out = dec.decode_pts(p, i as i64).unwrap_or_else(|e| panic!("{what} frame {i}: {e:?}"));
        out.extend(dec.flush());
        assert_eq!(out.len(), 1, "{what} frame {i}");
        assert_eq!(out[0].planes, want.planes, "{what} frame {i}: decoder output differs from the reconstruction");
    }
}

#[test]
fn encoder_tiles_decode_bit_exactly_key_and_p_frames() {
    for &(w, h) in &[(330u32, 200u32), (193, 131), (65, 300), (256, 130)] {
        for speed in [6u8, 8] {
            for tiles in [(0u8, 0u8), (1, 0), (0, 1), (1, 1), (2, 1)] {
                let c = encode_seq(w, h, 3, tiles, speed, 110, 3);
                check_decodes(&c, &format!("{w}x{h} speed {speed} tiles {tiles:?}"));
            }
        }
    }
}

#[test]
fn encoder_1080p_tiles_decode_bit_exactly_and_do_not_depend_on_threads() {
    for tiles in [(1u8, 0u8), (1, 1), (2, 1)] {
        let one = encode_seq(1920, 1080, 2, tiles, 8, 120, 1);
        check_decodes(&one, &format!("1080p tiles {tiles:?}"));
        for threads in [3usize, 8] {
            let many = encode_seq(1920, 1080, 2, tiles, 8, 120, threads);
            assert_eq!(one.packets, many.packets, "1080p tiles {tiles:?}: bytes differ between 1 and {threads} threads");
        }
    }
}

#[test]
fn encoder_tiles_do_not_depend_on_threads_at_speed_6() {
    for tiles in [(1u8, 1u8), (2, 1)] {
        let one = encode_seq(330, 200, 3, tiles, 6, 130, 1);
        for threads in [2usize, 7] {
            assert_eq!(one.packets, encode_seq(330, 200, 3, tiles, 6, 130, threads).packets, "tiles {tiles:?}, {threads} threads");
        }
    }
}

#[test]
fn encoder_tiled_streams_decode_on_videotoolbox_bit_exactly() {
    use studio_encode::mux::{Muxer, Options, av1c_from_sequence_header};
    if !studio_hw::Av1Decoder::supported() {
        eprintln!("SKIPPED: this machine's video hardware does not decode AV1");
        return;
    }
    let dir = std::env::temp_dir().join(format!("studio-av1-enc-tiles-enc-vt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for &(w, h, tiles, speed) in &[(1920u32, 1080u32, (1u8, 0u8), 8u8), (1920, 1080, (1, 1), 8), (1920, 1080, (2, 1), 8), (1920, 1080, (2, 1), 6), (330, 200, (1, 1), 6), (193, 131, (1, 0), 6)] {
        let c = encode_seq(w, h, 3, tiles, speed, 110, 4);
        let seq_obu_len = 2 + c.packets[0][1] as usize;
        let info = av1c_from_sequence_header(&c.packets[0][..seq_obu_len]).unwrap();
        let path = dir.join(format!("enc-{w}x{h}-{}{}-s{speed}.mp4", tiles.0, tiles.1));
        let mut mux = Muxer::create(&path, Options::mp4_av1()).unwrap();
        let track = mux.add_av1_video(info.track_info(30, 1, 1_000_000)).unwrap();
        for p in &c.packets {
            mux.write_temporal_unit(track, p).unwrap();
        }
        mux.finish().unwrap();
        let mut demux = studio_mp4::open(&path).unwrap();
        let movie = demux.movie().clone();
        let v = &movie.tracks[0];
        let studio_mp4::Codec::Av1 { config } = &v.codec else { panic!("not AV1: {:?}", v.codec) };
        let mut hw = studio_hw::Av1Decoder::new(&config.raw, w, h).expect("a hardware AV1 decoder");
        let mut got = Vec::new();
        for (i, s) in v.samples.iter().enumerate() {
            got.extend(hw.decode(&demux.read_sample(s).unwrap(), i as i64).unwrap());
        }
        got.extend(hw.flush());
        got.sort_by_key(|d| d.sample);
        assert_eq!(got.len(), c.recon.len(), "{w}x{h}");
        let mut exact = true;
        for (d, want) in got.iter().zip(&c.recon) {
            exact &= [(&d.y, &want.planes[0]), (&d.u, &want.planes[1]), (&d.v, &want.planes[2])].iter().all(|(a, b)| a.len() == b.len() && a.iter().zip(b.iter()).all(|(&x, &y)| x as u16 == y));
        }
        eprintln!("{w}x{h} tiles {tiles:?} speed {speed}: VideoToolbox matches the reconstruction bit for bit: {exact}");
        assert!(exact, "{w}x{h} tiles {tiles:?} speed {speed}: VideoToolbox output differs");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bytes at equal PSNR and speed, for the report: `cargo test --release -p studio-av1-enc --test
/// tiles -- --ignored --nocapture bench_tiling`. Key + 2 P frames of a 1080p pan, speed 6, all cores.
#[test]
#[ignore = "benchmark: run in release mode and read the printed numbers"]
fn bench_tiling_cost_and_speed_1080p() {
    let (w, h, n) = (1920u32, 1080u32, 3u32);
    let src: Vec<Frame> = (0..n).map(|i| pan_frame(w, h, i)).collect();
    let samples = (w * h + 2 * (w / 2) * (h / 2)) as f64 * n as f64;
    let sse = |a: &[u16], b: &[u16]| -> u64 { a.iter().zip(b).map(|(&x, &y)| (x as i64 - y as i64).pow(2) as u64).sum() };
    let speed: u8 = std::env::var("BENCH_SPEED").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let layouts = [(0u8, 0u8), (1, 0), (1, 1), (2, 1), (2, 2), (3, 1)];
    let mut curves: Vec<Vec<(f64, f64)>> = Vec::new();
    for &tiles in &layouts {
        let mut curve = Vec::new();
        for q in [90u8, 120, 150, 180] {
            let mut enc = Encoder::new(Config { speed, tiles, threads: cores, rate: RateMode::Cqp(q), keyint: 240, ..Config::new(w, h) }).unwrap();
            let (mut bytes, mut err) = (0usize, 0u64);
            for (i, f) in src.iter().enumerate() {
                bytes += enc.push_frame(f, i as i64).unwrap()[0].data.len();
                err += (0..3).map(|c| sse(&enc.reconstruction().unwrap().planes[c], &f.planes[c])).sum::<u64>();
            }
            curve.push((10.0 * (255.0f64 * 255.0 * samples / err as f64).log10(), bytes as f64));
        }
        eprintln!("tiles {tiles:?}: (PSNR, bytes) {curve:?}");
        curve.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        curves.push(curve);
    }
    let at = |curve: &[(f64, f64)], psnr: f64| -> Option<f64> {
        curve.windows(2).find_map(|w| {
            let ((p0, b0), (p1, b1)) = (w[0], w[1]);
            (p0.min(p1)..=p0.max(p1)).contains(&psnr).then(|| (b0.ln() + (b1.ln() - b0.ln()) * (psnr - p0) / (p1 - p0)).exp())
        })
    };
    for (k, &tiles) in layouts.iter().enumerate().skip(1) {
        let r: Vec<f64> = curves[0].iter().filter_map(|&(p, b)| at(&curves[k], p).map(|n| n / b)).collect();
        eprintln!("tiles {tiles:?} ({} tiles): bytes at equal PSNR = {:.2}% of one tile ({} comparable points)", (1usize << tiles.0).min(30) * (1usize << tiles.1).min(17), 100.0 * r.iter().sum::<f64>() / r.len().max(1) as f64, r.len());
    }
    // speed: fps over the key frame and over the P frames, at 1 thread and at all cores
    for &tiles in &layouts {
        for threads in [1usize, cores] {
            let mut enc = Encoder::new(Config { speed, tiles, threads, rate: RateMode::Cqp(120), keyint: 240, ..Config::new(w, h) }).unwrap();
            let t = std::time::Instant::now();
            enc.push_frame(&src[0], 0).unwrap();
            let key = t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            for (i, f) in src.iter().enumerate().skip(1) {
                enc.push_frame(f, i as i64).unwrap();
            }
            let p = t.elapsed().as_secs_f64() / (n - 1) as f64;
            eprintln!("speed {speed} tiles {tiles:?} threads {threads:2}: key {:.2} fps, P {:.2} fps", 1.0 / key, 1.0 / p);
        }
    }
}
