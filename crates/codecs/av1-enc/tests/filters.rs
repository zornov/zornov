// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M9: in-loop filters. The encoder's reconstruction must equal the decoder's output with each
//! filter on, and each filter must pay for itself.

use studio_av1::Decoder;
use studio_av1_enc::color::from_rgba8;
use studio_av1_enc::filters::FilterSet;
use studio_av1_enc::{Config, Encoder, Frame, RateMode};

fn noise(x: u32, y: u32, t: u32) -> i32 {
    let mut v = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca6b) ^ t.wrapping_mul(0xc2b2_ae35);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2c1b_3c6d);
    v ^= v >> 12;
    (v % 21) as i32 - 10
}

fn picture(w: u32, h: u32, shift: u32) -> Frame {
    // smooth gradients with a moving edge: blocking shows at coarse quantisers
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let edge = if (x + shift * 3) % 40 < 20 { 60 } else { 0 };
            [((x * 2 + y + shift * 2) as i32 % 256 + noise(x, y, shift)).clamp(0, 255) as u8, ((y * 3 % 200 + edge) as i32 + noise(y, x, shift)).clamp(0, 255) as u8, ((x * y / 20 % 256) as i32 + noise(x + 7, y, shift)).clamp(0, 255) as u8, 255]
        })
        .collect();
    from_rgba8(&rgba, w, h, 8, false)
}

fn sse(a: &[u16], b: &[u16]) -> u64 {
    a.iter().zip(b).map(|(&x, &y)| (x as i64 - y as i64).pow(2) as u64).sum()
}

/// Encodes `n` frames; (packet bytes, squared error against the source, reconstruction == decode).
fn run(filters: FilterSet, q: u8, n: u32) -> (usize, u64) {
    let (w, h) = (130, 70);
    let mut e = Encoder::new(Config { speed: 6, keyint: 4, rate: RateMode::Cqp(q), ..Config::new(w, h) }).unwrap().with_filters(filters);
    let mut dec = Decoder::new();
    let (mut bytes, mut err) = (0, 0u64);
    for i in 0..n {
        let src = picture(w, h, i);
        let p = e.push_frame(&src, i as i64).unwrap().remove(0);
        bytes += p.data.len();
        let mut out = dec.decode_pts(&p.data, p.pts).unwrap();
        out.extend(dec.flush());
        assert_eq!(out[0].planes, e.reconstruction().unwrap().planes, "frame {i}");
        err += (0..3).map(|c| sse(&out[0].planes[c], &src.planes[c])).sum::<u64>();
    }
    (bytes, err)
}

const OFF: FilterSet = FilterSet { deblock: false, cdef: false, restoration: false };
const DEBLOCK: FilterSet = FilterSet { deblock: true, cdef: false, restoration: false };
const BOTH: FilterSet = FilterSet { deblock: true, cdef: true, restoration: false };
const ALL: FilterSet = FilterSet { deblock: true, cdef: true, restoration: true };

#[test]
fn deblocked_frames_decode_to_the_encoders_reconstruction_and_beat_unfiltered() {
    let off = run(OFF, 220, 6);
    let on = run(DEBLOCK, 220, 6);
    println!("deblock off {off:?} on {on:?}");
    assert!(on.1 < off.1, "deblocking did not reduce the error: {on:?} vs {off:?}");
}

#[test]
fn cdef_frames_decode_to_the_encoders_reconstruction_and_beat_deblocking_alone() {
    for q in [120u8, 200] {
        let deblock = run(DEBLOCK, q, 6);
        let both = run(BOTH, q, 6);
        println!("q {q}: deblock {deblock:?} deblock+cdef {both:?}");
        assert!(both.1 < deblock.1, "CDEF did not reduce the error at q {q}: {both:?} vs {deblock:?}");
    }
}

#[test]
fn restoration_frames_decode_to_the_encoders_reconstruction_and_beat_cdef_alone() {
    let mut gained = false;
    for q in [50u8, 120, 200] {
        let both = run(BOTH, q, 4);
        let all = run(ALL, q, 4);
        println!("q {q}: deblock+cdef {both:?} +restoration {all:?}");
        // a unit only keeps a filter when it pays for its bits, so the error never gets worse
        assert!(all.1 <= both.1, "restoration made the error worse at q {q}: {all:?} vs {both:?}");
        gained |= all.1 < both.1;
    }
    assert!(gained, "restoration never ran");
}

/// Bytes (log-linear in PSNR between the measured points) a curve needs to reach `psnr`.
fn bytes_at(curve: &[(f64, f64)], psnr: f64) -> Option<f64> {
    curve.windows(2).find_map(|w| {
        let ((p0, b0), (p1, b1)) = (w[0], w[1]);
        (p0.min(p1)..=p0.max(p1)).contains(&psnr).then(|| (b0.ln() + (b1.ln() - b0.ln()) * (psnr - p0) / (p1 - p0)).exp())
    })
}

/// The gate: each filter, added in turn, saves bytes at equal PSNR on moving content.
#[test]
fn every_filter_saves_bytes_at_equal_psnr() {
    let samples = (130 * 70 + 2 * 65 * 35) as f64 * 6.0;
    let qs = [120u8, 150, 180, 210, 240];
    let curve = |set: FilterSet| -> Vec<(f64, f64)> {
        let mut c: Vec<(f64, f64)> = qs
            .iter()
            .map(|&q| {
                let (bytes, err) = run(set, q, 6);
                (10.0 * (255.0f64 * 255.0 * samples / err as f64).log10(), bytes as f64)
            })
            .collect();
        c.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        c
    };
    let (off, deblock, both, all) = (curve(OFF), curve(DEBLOCK), curve(BOTH), curve(ALL));
    println!("off {off:?}\ndeblock {deblock:?}\ndeblock+cdef {both:?}\nall {all:?}");
    let mean_ratio = |new: &[(f64, f64)], old: &[(f64, f64)]| -> f64 {
        let r: Vec<f64> = old.iter().filter_map(|&(p, b)| bytes_at(new, p).map(|n| n / b)).collect();
        assert!(r.len() >= 3, "too few comparable operating points");
        r.iter().sum::<f64>() / r.len() as f64
    };
    let (d, c, l) = (mean_ratio(&deblock, &off), mean_ratio(&both, &deblock), mean_ratio(&all, &both));
    println!("bytes at equal PSNR: deblock {:.1}% of off, +cdef {:.1}% of deblock, +restoration {:.1}% of deblock+cdef; all {:.1}% of off", d * 100.0, c * 100.0, l * 100.0, mean_ratio(&all, &off) * 100.0);
    assert!(d < 1.0, "deblocking does not pay: {d}");
    assert!(c < 1.0, "CDEF does not pay: {c}");
    assert!(l <= 1.0, "restoration does not pay: {l}");
}

/// Where the time goes at 1080p, for the profile report: `cargo test --release -p studio-av1-enc
/// --test filters -- --ignored --nocapture profile_1080p`. One key frame and a few P frames of a pan.
#[test]
#[ignore = "benchmark: run in release mode and read the printed numbers"]
fn profile_1080p() {
    use studio_av1_enc::filters::profile;
    let (w, h) = (1920usize, 1080usize);
    let threads: usize = std::env::var("PROFILE_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let n: usize = std::env::var("PROFILE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    let frames: Vec<Frame> = (0..n as u32)
        .map(|i| {
            let rgba: Vec<u8> = (0..(w * h) as u32)
                .flat_map(|j| {
                    let (x, y) = (j % w as u32 + 3 * i, j / w as u32 + 2 * i);
                    let edge = if (x / 24 + y / 40) % 2 == 0 { 50 } else { 0 };
                    [((x / 8 + y / 12) as i32 % 256 + noise(x, y, 0)).clamp(0, 255) as u8, ((y / 4 % 200 + edge) as i32 + noise(y, x, 1)).clamp(0, 255) as u8, ((x * y / 4000 % 256) as i32 + noise(x + 7, y, 2)).clamp(0, 255) as u8, 255]
                })
                .collect();
            from_rgba8(&rgba, w as u32, h as u32, 8, false)
        })
        .collect();
    for speed in [8u8, 6] {
        let mut e = Encoder::new(Config { speed, keyint: 240, rate: RateMode::Cqp(140), threads, ..Config::new(w as u32, h as u32) }).unwrap();
        for (label, range) in [("key", 0..1), ("P", 1..n)] {
            let _ = profile::take();
            let t = std::time::Instant::now();
            let mut bytes = 0;
            for i in range.clone() {
                bytes += e.push_frame(&frames[i], i as i64).unwrap()[0].data.len();
            }
            let total = t.elapsed().as_secs_f64() * 1000.0 / range.len() as f64;
            let s = profile::take();
            let per = |ns: u64| ns as f64 / 1e6 / range.len() as f64;
            let filt = per(s.deblock_search_ns) + per(s.cdef_search_ns) + per(s.lr_search_ns) + per(s.apply_ns);
            eprintln!(
                "speed {speed} {label:3} threads {threads}: {total:8.1} ms/frame ({} B) | deblock search {:7.1} | cdef search {:7.1} | restoration search {:7.1} | final apply {:6.1} | rest (mode+motion+entropy+rewrite) {:7.1}",
                bytes / range.len(), per(s.deblock_search_ns), per(s.cdef_search_ns), per(s.lr_search_ns), per(s.apply_ns), total - filt
            );
        }
    }
}

/// Splits the non-filter time of a 1080p frame (threads 1) into the mode and motion decisions and
/// the entropy writing, by driving the stages directly like `Encoder` does. A second write of the
/// same decisions stands for the rewrite after the filter search.
#[test]
#[ignore = "benchmark: run in release mode and read the printed numbers"]
fn profile_1080p_stages() {
    use std::sync::Arc;
    use studio_av1_enc::mode::{ModeDecider, PartitionDecider};
    use studio_av1_enc::motion::InterFrameCoder;
    use studio_av1_enc::recon::{FrameBuf, QuantInfo, RefStore};
    use studio_av1_enc::syntax::TileWriter;
    use studio_av1_enc::types::{SbPos, SeqParams};
    let (w, h) = (1920u32, 1080u32);
    let mk = |i: u32| {
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|j| {
                let (x, y) = (j % w + 3 * i, j / w + 2 * i);
                let edge = if (x / 24 + y / 40) % 2 == 0 { 50 } else { 0 };
                [((x / 8 + y / 12) as i32 % 256 + noise(x, y, 0)).clamp(0, 255) as u8, ((y / 4 % 200 + edge) as i32 + noise(y, x, 1)).clamp(0, 255) as u8, ((x * y / 4000 % 256) as i32 + noise(x + 7, y, 2)).clamp(0, 255) as u8, 255]
            })
            .collect();
        from_rgba8(&rgba, w, h, 8, false)
    };
    let (f0, f1) = (mk(0), mk(1));
    let seq = SeqParams { width: w, height: h, bit_depth: 8, fps: (30, 1), profile: 0, level_idx: 8, use_128x128_superblock: false, enable_cdef: true, enable_restoration: true, enable_order_hint: true, color_primaries: 1, transfer_characteristics: 1, matrix_coefficients: 1, full_range: false };
    let (mi_cols, mi_rows) = (2 * ((w + 7) >> 3), 2 * ((h + 7) >> 3));
    let q = 140u8;
    for speed in [8u8, 6] {
        let min_n4 = if speed >= 7 { 4 } else { 2 };
        // key frame
        let mut buf = FrameBuf::new(w, h, 8);
        buf.quant = QuantInfo { base_q_idx: q, ..Default::default() };
        buf.enable_intra_edge_filter = false;
        let mut tw = TileWriter::new(&seq, q, mi_cols, mi_rows);
        let mut dec = PartitionDecider { base_q_idx: q, min_n4 };
        let (mut t_dec, mut t_wr) = (0.0, 0.0);
        let mut sbs = Vec::new();
        for row in 0..mi_rows.div_ceil(16) {
            for col in 0..mi_cols.div_ceil(16) {
                let pos = SbPos { col, row };
                let t = std::time::Instant::now();
                let sb = dec.decide_sb(&f0, &mut buf, pos);
                t_dec += t.elapsed().as_secs_f64();
                let t = std::time::Instant::now();
                tw.write_sb(pos, &sb);
                t_wr += t.elapsed().as_secs_f64();
                sbs.push((pos, sb));
            }
        }
        let t = std::time::Instant::now();
        let mut tw2 = TileWriter::new(&seq, q, mi_cols, mi_rows);
        for (pos, sb) in &sbs {
            tw2.write_sb(*pos, sb);
        }
        let t_rewrite = t.elapsed().as_secs_f64();
        eprintln!("speed {speed} key: intra mode+partition search {:7.1} ms | entropy write {:6.1} ms | rewrite {:6.1} ms", t_dec * 1e3, t_wr * 1e3, t_rewrite * 1e3);
        // inter frame from that reconstruction
        let mut refs = RefStore::default();
        refs.refresh(0xFF, Arc::new(buf));
        let mut buf = FrameBuf::new(w, h, 8);
        buf.quant = QuantInfo { base_q_idx: q, ..Default::default() };
        buf.enable_intra_edge_filter = false;
        let mut tw = TileWriter::new(&seq, q, mi_cols, mi_rows);
        let mut coder = InterFrameCoder::new(&f1, &refs, [0u8; 7], 0, speed <= 6).with_min_block(min_n4);
        let (mut t_dec, mut t_wr) = (0.0, 0.0);
        let mut sbs = Vec::new();
        for row in 0..mi_rows.div_ceil(16) {
            for col in 0..mi_cols.div_ceil(16) {
                let pos = SbPos { col, row };
                let t = std::time::Instant::now();
                let sb = coder.decide_sb(&mut buf, pos);
                t_dec += t.elapsed().as_secs_f64();
                let t = std::time::Instant::now();
                tw.write_inter_sb(pos, &sb);
                t_wr += t.elapsed().as_secs_f64();
                sbs.push((pos, sb));
            }
        }
        let t = std::time::Instant::now();
        let mut tw2 = TileWriter::new(&seq, q, mi_cols, mi_rows);
        for (pos, sb) in &sbs {
            tw2.write_inter_sb(*pos, sb);
        }
        let t_rewrite = t.elapsed().as_secs_f64();
        eprintln!("speed {speed} P:   mode+motion search {:7.1} ms | entropy write {:6.1} ms | rewrite {:6.1} ms", t_dec * 1e3, t_wr * 1e3, t_rewrite * 1e3);
        if speed == 6 {
            use studio_av1_enc::filters::{apply, decoder_sequence, search_cdef_mt, search_deblock_mt, search_lr, FilterParams};
            let dseq = decoder_sequence(&studio_av1_enc::obu::write_sequence_header(&seq));
            let mi = tw2.mode_info();
            for threads in [1usize, 2, 4, 14] {
                let t = std::time::Instant::now();
                let lf = search_deblock_mt(&dseq, &buf, mi, &f1, threads);
                let t_lf = t.elapsed().as_secs_f64() * 1e3;
                let deblocked = apply(&dseq, &buf, mi, &FilterParams { lf_level: lf, cdef: None, lr: None });
                let t = std::time::Instant::now();
                let cdef = search_cdef_mt(&dseq, &deblocked, mi, &f1, q, threads);
                let t_cdef = t.elapsed().as_secs_f64() * 1e3;
                let t = std::time::Instant::now();
                let lr = search_lr(&dseq, &buf, mi, &f1, lf, cdef.as_ref(), q, threads);
                let t_lr = t.elapsed().as_secs_f64() * 1e3;
                eprintln!("speed 6 P filters at {threads:2} threads: deblock search {t_lf:6.1} ms (lf {lf:?}) | cdef search {t_cdef:6.1} ms | restoration search {t_lr:7.1} ms ({})", lr.is_some());
            }
        }
    }
}

/// The searches pick the same parameters whatever the worker count.
#[test]
fn filter_searches_do_not_depend_on_the_thread_count() {
    use studio_av1_enc::filters::{decoder_sequence, search_cdef_mt, search_deblock_mt, search_lr};
    use studio_av1_enc::mode::{ModeDecider, PartitionDecider};
    use studio_av1_enc::obu::write_sequence_header;
    use studio_av1_enc::recon::{FrameBuf, QuantInfo};
    use studio_av1_enc::syntax::TileWriter;
    use studio_av1_enc::types::{SbPos, SeqParams};
    let (w, h) = (210u32, 140u32);
    let src = picture(w, h, 2);
    let seq = SeqParams { width: w, height: h, bit_depth: 8, fps: (30, 1), profile: 0, level_idx: 0, use_128x128_superblock: false, enable_cdef: true, enable_restoration: true, enable_order_hint: true, color_primaries: 1, transfer_characteristics: 1, matrix_coefficients: 1, full_range: false };
    let dseq = decoder_sequence(&write_sequence_header(&seq));
    let (mi_cols, mi_rows) = (2 * ((w + 7) >> 3), 2 * ((h + 7) >> 3));
    for q in [100u8, 200] {
        let mut buf = FrameBuf::new(w, h, 8);
        buf.quant = QuantInfo { base_q_idx: q, ..Default::default() };
        buf.enable_intra_edge_filter = false;
        let mut tw = TileWriter::new(&seq, q, mi_cols, mi_rows);
        let mut dec = PartitionDecider { base_q_idx: q, min_n4: 2 };
        for row in 0..mi_rows.div_ceil(16) {
            for col in 0..mi_cols.div_ceil(16) {
                let pos = SbPos { col, row };
                let sb = dec.decide_sb(&src, &mut buf, pos);
                tw.write_sb(pos, &sb);
            }
        }
        let mi = tw.mode_info();
        let lf1 = search_deblock_mt(&dseq, &buf, mi, &src, 1);
        for t in [2usize, 8] {
            assert_eq!(search_deblock_mt(&dseq, &buf, mi, &src, t), lf1, "deblock at {t} threads, q {q}");
        }
        let deblocked = studio_av1_enc::filters::apply(&dseq, &buf, mi, &studio_av1_enc::filters::FilterParams { lf_level: lf1, cdef: None, lr: None });
        let cdef1 = search_cdef_mt(&dseq, &deblocked, mi, &src, q, 1);
        for t in [2usize, 8] {
            assert_eq!(search_cdef_mt(&dseq, &deblocked, mi, &src, q, t), cdef1, "cdef at {t} threads, q {q}");
        }
        let lr1 = search_lr(&dseq, &buf, mi, &src, lf1, cdef1.as_ref(), q, 1);
        for t in [2usize, 8] {
            assert_eq!(search_lr(&dseq, &buf, mi, &src, lf1, cdef1.as_ref(), q, t), lr1, "restoration at {t} threads, q {q}");
        }
        println!("q {q}: lf {lf1:?} cdef {} lr {}", cdef1.is_some(), lr1.is_some());
    }
}

/// Bytes at equal PSNR with the restoration probe on every unit, one in two and one in four, and
/// the time of the restoration search: `cargo test --release -p studio-av1-enc --test filters --
/// --ignored --nocapture probe_sampling`. Content: 640x360 pan with texture, threads 1.
#[test]
#[ignore = "benchmark: run in release mode and read the printed numbers"]
fn probe_sampling_cost_at_equal_psnr() {
    use studio_av1_enc::filters::{profile, set_probe_sampling, set_probe_small};
    let (w, h) = (640u32, 360u32);
    let frames: Vec<Frame> = (0..6u32)
        .map(|i| {
            let rgba: Vec<u8> = (0..w * h)
                .flat_map(|j| {
                    let (x, y) = (j % w + 3 * i, j / w + 2 * i);
                    let edge = if (x / 24 + y / 40) % 2 == 0 { 50 } else { 0 };
                    [((x / 3 + y / 5) as i32 % 256 + noise(x, y, 0)).clamp(0, 255) as u8, ((y / 2 % 200 + edge) as i32 + noise(y, x, 1)).clamp(0, 255) as u8, ((x * y / 900 % 256) as i32 + noise(x + 7, y, 2)).clamp(0, 255) as u8, 255]
                })
                .collect();
            from_rgba8(&rgba, w, h, 8, false)
        })
        .collect();
    let samples = (w * h + 2 * (w / 2) * (h / 2)) as f64 * frames.len() as f64;
    let mut curves = Vec::new();
    let settings = [(1usize, false), (4, false), (8, false), (1, true), (4, true), (8, true)];
    for (every, small) in settings {
        set_probe_sampling(every);
        set_probe_small(small);
        let _ = profile::take();
        let mut curve = Vec::new();
        for q in [100u8, 130, 160, 190, 220] {
            let mut e = Encoder::new(Config { speed: 6, keyint: 240, rate: RateMode::Cqp(q), threads: 1, ..Config::new(w, h) }).unwrap();
            let (mut bytes, mut err) = (0usize, 0u64);
            for (i, f) in frames.iter().enumerate() {
                bytes += e.push_frame(f, i as i64).unwrap()[0].data.len();
                err += (0..3).map(|c| sse(&e.reconstruction().unwrap().planes[c], &f.planes[c])).sum::<u64>();
            }
            curve.push((10.0 * (255.0f64 * 255.0 * samples / err as f64).log10(), bytes as f64));
        }
        let lr_ms = profile::take().lr_search_ns as f64 / 1e6;
        eprintln!("probe 1/{every} small={small}: restoration search {lr_ms:.0} ms total; curve {curve:?}");
        curve.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        curves.push(curve);
    }
    for (k, &(every, small)) in settings.iter().enumerate().skip(1) {
        let r: Vec<f64> = curves[0].iter().filter_map(|&(p, b)| bytes_at(&curves[k], p).map(|n| n / b)).collect();
        eprintln!("probe 1/{every} small={small}: bytes at equal PSNR = {:.2}% of probing every unit ({} points)", 100.0 * r.iter().sum::<f64>() / r.len() as f64, r.len());
    }
}
