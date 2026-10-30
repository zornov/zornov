// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Phase (a) gate, in-crate part: the public `Encoder` on 20+ test pictures (gradients, noise,
//! text-like edges, stripes, flat, odd sizes from 1x1 to 1921x1081). Every packet must decode
//! with `studio-av1` to the encoder's own reconstruction, bit for bit, and meet a PSNR floor for
//! its content class and quantiser. A bounded fuzz run over sizes and quantisers must not panic.
//! The MP4 test muxes the packets with the workspace muxer and decodes the file on the video
//! hardware (VideoToolbox). An ignored benchmark records the speed and the machine load.

use studio_av1::Decoder;
use studio_av1_enc::{Config, Encoder, Frame, RateMode};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

fn image(w: usize, h: usize, mut f: impl FnMut(usize, usize, usize) -> u8) -> Frame {
    let mut fr = Frame::new(w as u32, h as u32, 8);
    for p in 0..3 {
        let (pw, ph) = if p == 0 { (w, h) } else { (w.div_ceil(2), h.div_ceil(2)) };
        for y in 0..ph {
            for x in 0..pw {
                fr.planes[p][y * fr.stride[p] + x] = f(p, x, y) as u16;
            }
        }
    }
    fr
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Class {
    /// Smooth content: the 64x64 transforms of phase (a) code it well.
    Smooth,
    /// Detail finer than the 32 low-frequency coefficients a 64-point transform keeps.
    Detailed,
}

fn gradient(w: usize, h: usize) -> Frame {
    image(w, h, |p, x, y| match p {
        0 => ((x * 255 / w.max(1) + y * 255 / h.max(1)) / 2) as u8,
        1 => (64 + x * 128 / w.max(1)) as u8,
        _ => (200 - y * 128 / h.max(1)) as u8,
    })
}

fn noise(w: usize, h: usize, seed: u64) -> Frame {
    let mut rng = Rng(seed);
    image(w, h, |_, _, _| (96 + rng.next() % 64) as u8)
}

fn edges(w: usize, h: usize) -> Frame {
    image(w, h, |p, x, y| {
        let (cx, cy) = (w as i32 / 2, h as i32 / 2);
        let k = 1 + (p > 0) as i32;
        let (dx, dy) = (x as i32 - cx / k, y as i32 - cy / k);
        let disc = dx * dx + dy * dy < (h as i32 / 3).pow(2) / (1 + 3 * (p > 0) as i32);
        let stripes = if (x / 5 + y / 3) % 2 == 0 { 60 } else { 190 };
        if disc { 235 } else { stripes }
    })
}

/// Black text-like marks on a light page: thin strokes and small boxes.
fn text(w: usize, h: usize) -> Frame {
    image(w, h, |p, x, y| {
        if p > 0 {
            return 128;
        }
        let (cx, cy) = (x / 3, y / 8);
        let glyph = (cx * 7 + cy * 13) % 5;
        let on = match glyph {
            0 => x % 3 == 0,
            1 => y % 8 == 3,
            2 => x % 3 == 1 && y % 8 < 6,
            _ => false,
        };
        if on { 20 } else { 235 }
    })
}

fn stripes(w: usize, h: usize) -> Frame {
    image(w, h, |_, x, y| if ((x + y) / 6) % 2 == 0 { 70 } else { 180 })
}

fn photo_like(w: usize, h: usize, seed: u64) -> Frame {
    let mut rng = Rng(seed);
    image(w, h, |p, x, y| {
        let base = if p == 0 { (x * 160 / w.max(1) + y * 80 / h.max(1) + 30) as i32 } else { 120 + (x * 20 / w.max(1)) as i32 };
        (base + (rng.next() % 5) as i32 - 2).clamp(16, 235) as u8
    })
}

fn flat(w: usize, h: usize, v: u8) -> Frame {
    image(w, h, |_, _, _| v)
}

fn gate_images() -> Vec<(String, Frame, Class)> {
    let mut v: Vec<(String, Frame, Class)> = Vec::new();
    let mut add = |name: &str, f: Frame, c: Class| v.push((format!("{name} {}x{}", f.width, f.height), f, c));
    for &(w, h) in &[(64usize, 64usize), (66, 66), (130, 70), (640, 360)] {
        add("gradient", gradient(w, h), Class::Smooth);
    }
    add("noise", noise(64, 64, 1), Class::Detailed);
    add("noise", noise(66, 66, 2), Class::Detailed);
    add("noise", noise(200, 120, 3), Class::Detailed);
    add("edges", edges(64, 64), Class::Detailed);
    add("edges", edges(66, 66), Class::Detailed);
    add("edges", edges(200, 120), Class::Detailed);
    add("edges", edges(640, 360), Class::Detailed);
    add("text", text(128, 96), Class::Detailed);
    add("text", text(321, 177), Class::Detailed);
    add("stripes", stripes(100, 100), Class::Detailed);
    add("photo-like", photo_like(320, 180, 4), Class::Smooth);
    add("photo-like", photo_like(257, 129, 5), Class::Smooth);
    add("flat", flat(8, 8, 90), Class::Smooth);
    add("flat", flat(1, 1, 200), Class::Smooth);
    add("flat", flat(100, 60, 128), Class::Smooth);
    add("tiny", gradient(4, 4), Class::Smooth);
    add("tiny", gradient(9, 7), Class::Smooth);
    add("photo-like", photo_like(1920, 1080, 6), Class::Smooth);
    add("photo-like", photo_like(1921, 1081, 7), Class::Smooth);
    v
}

fn psnr(a: &[u16], b: &[u16]) -> f64 {
    let mse = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// The luma PSNR the content class must reach at `q` (measured values less a margin; see the
/// printed table of the gate test).
fn floor(class: Class, q: u8) -> f64 {
    match (class, q) {
        (Class::Smooth, 0..=40) => 42.0,
        (Class::Smooth, 41..=120) => 36.0,
        (Class::Smooth, _) => 30.0,
        // Fine detail is cut by the 64-point transform (it keeps 32x32 of 64x64 coefficients, so
        // noise stays near 24 dB at any quantiser) until a partition search picks smaller
        // blocks in phase (c): the floor only guards against garbage.
        (Class::Detailed, _) => 12.0,
    }
}

struct Outcome {
    bytes: usize,
    psnr: f64,
}

/// Encodes `src`, decodes the packet with the real decoder, and checks every plane against the
/// encoder's reconstruction.
fn run(name: &str, src: &Frame, q: u8, speed: u8) -> Outcome {
    let cfg = Config { rate: RateMode::Cqp(q), speed, ..Config::new(src.width, src.height) };
    let mut enc = Encoder::new(cfg).unwrap();
    let pkts = enc.push_frame(src, 0).unwrap();
    assert_eq!(pkts.len(), 1);
    // bounded: nothing here costs more than 3 bytes a sample
    assert!(pkts[0].data.len() < 3 * (src.width * src.height) as usize + 2048, "{name} q{q}: {} bytes", pkts[0].data.len());
    let mut dec = Decoder::new();
    let mut pics = dec.decode(&pkts[0].data).unwrap_or_else(|e| panic!("{name} q{q} speed {speed}: {e:?}"));
    pics.extend(dec.flush_result().unwrap());
    assert_eq!(pics.len(), 1, "{name} q{q}: one picture");
    let want = enc.reconstruction().unwrap();
    for p in 0..3 {
        assert!(pics[0].planes[p] == want.planes[p], "{name} q{q} speed {speed}: plane {p} differs from the encoder's reconstruction");
    }
    Outcome { bytes: pkts[0].data.len(), psnr: psnr(&pics[0].planes[0], &src.planes[0]) }
}

#[test]
fn gate_images_decode_bit_exactly_and_meet_the_psnr_floors() {
    let images = gate_images();
    assert!(images.len() >= 20, "{} images", images.len());
    let mut cases = 0;
    for (name, img, class) in &images {
        // the big pictures only at one quantiser and without the search, to keep the run short
        let big = img.width * img.height > 400_000;
        let qs: &[u8] = if big { &[100] } else { &[20, 100, 200] };
        for &q in qs {
            let o = run(name, img, q, if big { 9 } else { 6 });
            eprintln!("{name:22} q{q:3} {:?}: {:7} B  {:5.2} dB", class, o.bytes, o.psnr);
            assert!(o.psnr >= floor(*class, q), "{name} q{q}: PSNR {:.2} below the floor {}", o.psnr, floor(*class, q));
            cases += 1;
        }
    }
    eprintln!("{cases} encodes, all bit-exact");
}

#[test]
fn quality_improves_with_a_lower_quantiser() {
    let img = photo_like(320, 180, 11);
    let (a, b, c) = (run("p", &img, 30, 6), run("p", &img, 110, 6), run("p", &img, 220, 6));
    assert!(a.psnr > b.psnr && b.psnr > c.psnr, "{} {} {}", a.psnr, b.psnr, c.psnr);
    assert!(a.bytes > b.bytes && b.bytes > c.bytes);
}

#[test]
fn fuzz_lite_random_sizes_and_quantisers_never_panic() {
    let mut rng = Rng(2024);
    for i in 0..60 {
        let (w, h) = (1 + (rng.next() % 90) as usize, 1 + (rng.next() % 90) as usize);
        let q = 1 + (rng.next() % 255) as u8;
        let speed = [3u8, 6, 9][(rng.next() % 3) as usize];
        let img = match rng.next() % 4 {
            0 => gradient(w, h),
            1 => noise(w, h, i),
            2 => edges(w, h),
            _ => photo_like(w, h, i),
        };
        run(&format!("fuzz {i} {w}x{h}"), &img, q, speed);
    }
}

/// Speed and machine load, for the gate report: `cargo test --release -p studio-av1-enc --test
/// roundtrip -- --ignored --nocapture`. Single thread, 1080p.
#[test]
#[ignore = "benchmark: run in release mode and read the printed numbers"]
fn bench_1080p_single_thread() {
    let img = photo_like(1920, 1080, 9);
    let load = std::process::Command::new("sysctl").args(["-n", "vm.loadavg"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    eprintln!("vm.loadavg before: {load}");
    for speed in [9u8, 6] {
        let mut enc = Encoder::new(Config { rate: RateMode::Cqp(100), speed, keyint: 1, threads: 1, ..Config::new(1920, 1080) }).unwrap();
        let n = 5;
        let t = std::time::Instant::now();
        let mut bytes = 0;
        for i in 0..n {
            bytes += enc.push_frame(&img, i).unwrap()[0].data.len();
        }
        let secs = t.elapsed().as_secs_f64();
        eprintln!("1080p key frames, speed {speed}: {:.2} fps over {n} frames, {} B/frame", n as f64 / secs, bytes / n as usize);
    }
    let load = std::process::Command::new("sysctl").args(["-n", "vm.loadavg"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    eprintln!("vm.loadavg after: {load}");
}

/// The packets, in an MP4 written by the workspace muxer, read back by the workspace demuxer,
/// decode on the video hardware to the encoder's reconstruction.
#[test]
fn mp4_decodes_on_videotoolbox_bit_exactly() {
    use studio_encode::mux::{Muxer, Options, av1c_from_sequence_header};
    if !studio_hw::Av1Decoder::supported() {
        eprintln!("SKIPPED: this machine's video hardware does not decode AV1");
        return;
    }
    let dir = std::env::temp_dir().join(format!("studio-av1-enc-vt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for &(w, h) in &[(160u32, 96u32), (130, 70), (66, 66)] {
        let imgs = [gradient(w as usize, h as usize), photo_like(w as usize, h as usize, 3), edges(w as usize, h as usize)];
        let mut enc = Encoder::new(Config { rate: RateMode::Cqp(80), speed: 6, ..Config::new(w, h) }).unwrap();
        let (mut pkts, mut recon) = (Vec::new(), Vec::new());
        for (i, img) in imgs.iter().enumerate() {
            pkts.extend(enc.push_frame(img, i as i64).unwrap());
            recon.push(enc.reconstruction().unwrap().clone());
        }
        // the muxer's view of the stream: the sequence header comes from the first packet
        let seq_obu_len = 2 + pkts[0].data[1] as usize;
        let info = av1c_from_sequence_header(&pkts[0].data[..seq_obu_len]).unwrap();
        assert_eq!(info.av1c, enc.av1c(), "the muxer derives the av1C the encoder reports");
        let path = dir.join(format!("clip-{w}x{h}.mp4"));
        let mut mux = Muxer::create(&path, Options::mp4_av1()).unwrap();
        let track = mux.add_av1_video(info.track_info(30, 1, 1_000_000)).unwrap();
        for p in &pkts {
            mux.write_temporal_unit(track, &p.data).unwrap();
        }
        mux.finish().unwrap();

        let mut demux = studio_mp4::open(&path).unwrap();
        let movie = demux.movie().clone();
        let v = &movie.tracks[0];
        assert_eq!(&v.fourcc, b"av01");
        assert_eq!((v.width, v.height), (w, h));
        // the edges picture has nothing in common with the photo before it: a scene cut, so a key frame
        assert_eq!(v.samples.iter().map(|s| s.sync).collect::<Vec<_>>(), [true, false, true], "the key frames, and only they, are sync samples");
        let studio_mp4::Codec::Av1 { config } = &v.codec else { panic!("not AV1: {:?}", v.codec) };
        let mut hw = studio_hw::Av1Decoder::new(&config.raw, w, h).expect("a hardware AV1 decoder");
        let mut got = Vec::new();
        for (i, s) in v.samples.iter().enumerate() {
            let data = demux.read_sample(s).unwrap();
            got.extend(hw.decode(&data, i as i64).unwrap());
        }
        got.extend(hw.flush());
        assert_eq!(got.len(), imgs.len(), "{w}x{h}: every picture comes back");
        got.sort_by_key(|d| d.sample);
        let mut exact = true;
        for (d, want) in got.iter().zip(&recon) {
            assert_eq!((d.width, d.height), (w, h));
            for (plane, ref_plane) in [(&d.y, &want.planes[0]), (&d.u, &want.planes[1]), (&d.v, &want.planes[2])] {
                assert_eq!(plane.len(), ref_plane.len());
                exact &= plane.iter().zip(ref_plane).all(|(&a, &b)| a as u16 == b);
            }
        }
        eprintln!("{w}x{h}: VideoToolbox decode of the MP4 matches the reconstruction bit for bit: {exact}");
        assert!(exact, "{w}x{h}: VideoToolbox output differs from the encoder's reconstruction");
    }
}

// ---- inter frames (phase (b) gate) ----

/// A smooth textured canvas, sampled bilinearly so that pans can be fractional.
fn canvas(x: f64, y: f64) -> f64 {
    90.0 + 45.0 * (x * 0.045).sin() * (y * 0.035).cos() + 30.0 * ((x + 2.0 * y) * 0.021).sin() + 12.0 * (x * 0.17).sin() * (y * 0.13).sin()
}

#[derive(Clone, Copy, Debug)]
enum Motion {
    /// The whole picture pans by `(dx, dy)` samples a frame (fractions allowed).
    Pan(f64, f64),
    /// A bright square crosses a static textured background.
    Object,
}

fn moving_sequence(motion: Motion, w: usize, h: usize, n: usize) -> Vec<Frame> {
    (0..n)
        .map(|i| {
            image(w, h, |p, x, y| {
                let k = if p == 0 { 1.0 } else { 2.0 };
                let (fx, fy) = (x as f64 * k, y as f64 * k);
                let v = match motion {
                    Motion::Pan(dx, dy) => canvas(fx + dx * i as f64, fy + dy * i as f64),
                    Motion::Object => {
                        let (ox, oy) = (20.0 + 6.0 * i as f64, 30.0 + 2.0 * i as f64);
                        if fx >= ox && fx < ox + 40.0 && fy >= oy && fy < oy + 40.0 { 220.0 } else { canvas(fx, fy) }
                    }
                };
                let v = if p == 0 { v } else { 128.0 + (v - 90.0) * 0.3 };
                v.round().clamp(16.0, 235.0) as u8
            })
        })
        .collect()
}

/// Encodes the frames (key frame every `keyint`), checks that every decoded picture equals the
/// encoder's reconstruction, and returns the total bytes and the mean luma PSNR.
fn code_sequence(frames: &[Frame], q: u8, speed: u8, keyint: u32) -> (usize, f64, usize) {
    let (w, h) = (frames[0].width, frames[0].height);
    let mut enc = Encoder::new(Config { rate: RateMode::Cqp(q), speed, keyint, ..Config::new(w, h) }).unwrap();
    let mut dec = Decoder::new();
    let (mut bytes, mut psnr_sum, mut keys) = (0, 0.0, 0);
    for (i, f) in frames.iter().enumerate() {
        let p = enc.push_frame(f, i as i64).unwrap().remove(0);
        bytes += p.data.len();
        keys += p.keyframe as usize;
        let mut pics = dec.decode(&p.data).unwrap_or_else(|e| panic!("frame {i}: {e:?}"));
        pics.extend(dec.flush_result().unwrap());
        assert_eq!(pics.len(), 1);
        let want = enc.reconstruction().unwrap();
        for plane in 0..3 {
            assert!(pics[0].planes[plane] == want.planes[plane], "frame {i} (key {}) plane {plane} differs from the reconstruction", p.keyframe);
        }
        psnr_sum += psnr(&pics[0].planes[0], &f.planes[0]);
    }
    (bytes, psnr_sum / frames.len() as f64, keys)
}

/// The bytes the all-intra curve needs to reach `psnr`, by log-linear interpolation between its
/// operating points; `None` outside the curve.
fn intra_bytes_at(curve: &[(f64, f64)], psnr: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    for a in curve {
        for b in curve {
            if a.1 < psnr && psnr <= b.1 {
                let t = (psnr - a.1) / (b.1 - a.1);
                let v = (a.0.ln() + t * (b.0.ln() - a.0.ln())).exp();
                // the cheapest way the intra curve reaches this quality
                best = Some(best.map_or(v, |x: f64| x.min(v)));
            }
        }
    }
    best
}

/// Speed of the P frames, for the gate report (same command as the key frame benchmark).
#[test]
#[ignore = "benchmark: run in release mode and read the printed numbers"]
fn bench_1080p_p_frames_single_thread() {
    let frames = moving_sequence(Motion::Pan(3.0, 2.0), 1920, 1080, 6);
    let load = || std::process::Command::new("sysctl").args(["-n", "vm.loadavg"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    eprintln!("vm.loadavg before: {}", load());
    for speed in [9u8, 6] {
        let mut enc = Encoder::new(Config { rate: RateMode::Cqp(100), speed, threads: 1, ..Config::new(1920, 1080) }).unwrap();
        enc.push_frame(&frames[0], 0).unwrap();
        let t = std::time::Instant::now();
        let mut bytes = 0;
        for (i, f) in frames.iter().enumerate().skip(1) {
            bytes += enc.push_frame(f, i as i64).unwrap()[0].data.len();
        }
        let secs = t.elapsed().as_secs_f64();
        eprintln!("1080p P frames, speed {speed}: {:.2} fps over {} frames, {} B/frame", (frames.len() - 1) as f64 / secs, frames.len() - 1, bytes / (frames.len() - 1));
    }
    eprintln!("vm.loadavg after: {}", load());
}

/// The phase (b) gate: on moving content, key frame plus P frames spend clearly fewer bytes than
/// all-intra coding at the same PSNR. Every frame is also checked bit-exact against the decoder.
#[test]
fn p_frames_save_bytes_on_moving_content_at_equal_psnr() {
    let mut ratios = Vec::new();
    for (name, motion) in [("pan 3,2", Motion::Pan(3.0, 2.0)), ("pan 1.25,0.5", Motion::Pan(1.25, 0.5)), ("pan 0,-4.5", Motion::Pan(0.0, -4.5)), ("object", Motion::Object)] {
        let frames = moving_sequence(motion, 160, 96, 6);
        // the all-intra rate-distortion curve over a grid of quantisers
        let curve: Vec<(f64, f64)> = (10..=250).step_by(40).map(|q| {
            let (b, p, _) = code_sequence(&frames, q as u8, 6, 1);
            (b as f64, p)
        }).collect();
        for q in [40u8, 100, 180] {
            let (p_bytes, p_psnr, keys) = code_sequence(&frames, q, 6, 240);
            assert_eq!(keys, 1);
            let (same_q_intra, same_q_psnr, _) = code_sequence(&frames, q, 6, 1);
            let at = intra_bytes_at(&curve, p_psnr);
            let ratio = at.map(|b| p_bytes as f64 / b);
            eprintln!(
                "{name:13} q{q:3}: key+P {p_bytes:6} B {p_psnr:5.2} dB | all-intra at the same q {same_q_intra:6} B {same_q_psnr:5.2} dB | all-intra at {p_psnr:5.2} dB: {} | ratio {}",
                at.map_or("-".to_string(), |b| format!("{b:.0} B")),
                ratio.map_or("-".to_string(), |r| format!("{:.1}%", 100.0 * r))
            );
            if let Some(r) = ratio {
                ratios.push(r);
                assert!(r < 0.8, "{name} q{q}: key+P needs {:.0}% of the all-intra bytes at equal PSNR", 100.0 * r);
            }
        }
    }
    assert!(ratios.len() >= 8, "too few comparable operating points ({})", ratios.len());
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    eprintln!("mean bytes at equal PSNR: {:.1}% of all-intra over {} operating points", 100.0 * mean, ratios.len());
    assert!(mean < 0.6, "mean ratio {mean:.2}");
}

fn code_sequence_with(frames: &[Frame], q: u8, speed: u8, set: studio_av1_enc::filters::FilterSet) -> (usize, f64) {
    let (w, h) = (frames[0].width, frames[0].height);
    let mut enc = Encoder::new(Config { rate: RateMode::Cqp(q), speed, keyint: 240, ..Config::new(w, h) }).unwrap().with_filters(set);
    let (mut bytes, mut psnr_sum) = (0, 0.0);
    for (i, f) in frames.iter().enumerate() {
        bytes += enc.push_frame(f, i as i64).unwrap().remove(0).data.len();
        psnr_sum += psnr(&enc.reconstruction().unwrap().planes[0], &f.planes[0]);
    }
    (bytes, psnr_sum / frames.len() as f64)
}

/// Bytes a curve of `(psnr, bytes)` points (sorted by PSNR) needs to reach `psnr`, log-linear
/// between the points; `None` outside the curve.
fn curve_bytes_at(curve: &[(f64, f64)], psnr: f64) -> Option<f64> {
    curve.windows(2).find_map(|w| {
        let ((p0, b0), (p1, b1)) = (w[0], w[1]);
        (p0..=p1).contains(&psnr).then(|| (b0.ln() + (b1.ln() - b0.ln()) * (psnr - p0) / (p1 - p0)).exp())
    })
}

/// A slower speed must not cost bytes at equal PSNR: speed 6 searches more than speed 8 (full-pel
/// motion search and loop restoration), so on moving content it may only match or beat it.
#[test]
fn speed_6_needs_no_more_bytes_than_speed_8_at_equal_psnr() {
    let set = studio_av1_enc::filters::FilterSet::default();
    let mut ratios = Vec::new();
    for (name, motion) in [("pan 3,2", Motion::Pan(3.0, 2.0)), ("pan 1.25,0.5", Motion::Pan(1.25, 0.5)), ("object", Motion::Object)] {
        let frames = moving_sequence(motion, 320, 192, 6);
        let curve = |speed: u8| -> Vec<(f64, f64)> {
            let mut c: Vec<(f64, f64)> = [60u8, 100, 140, 180].iter().map(|&q| {
                let (b, p) = code_sequence_with(&frames, q, speed, set);
                (p, b as f64)
            }).collect();
            c.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            c
        };
        let (fast, slow) = (curve(8), curve(6));
        for &(p, b) in &fast {
            if let Some(s) = curve_bytes_at(&slow, p) {
                eprintln!("{name:13} {p:5.2} dB: speed 8 {b:6.0} B, speed 6 {s:6.0} B, ratio {:.3}", s / b);
                ratios.push(s / b);
            }
        }
    }
    assert!(ratios.len() >= 5, "too few comparable operating points ({})", ratios.len());
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    eprintln!("speed 6 needs {:.1}% of speed 8's bytes at equal PSNR over {} points", 100.0 * mean, ratios.len());
    assert!(mean <= 1.0, "speed 6 costs {:.1}% of speed 8's bytes", 100.0 * mean);
    assert!(ratios.iter().all(|&r| r <= 1.05), "speed 6 loses more than 5% at some operating point: {ratios:?}");
}

#[test]
#[ignore = "exploration: bytes at fixed PSNR targets (AV1LAMBDA sets the RD multiplier scale)"]
fn explore_lambda() {
    let set = studio_av1_enc::filters::FilterSet::default();
    let mut seqs: Vec<(String, Vec<Frame>)> = gate_images().into_iter().map(|(n, f, _)| (n, vec![f])).collect();
    seqs.push(("pan".into(), moving_sequence(Motion::Pan(3.0, 2.0), 160, 96, 4)));
    seqs.push(("object".into(), moving_sequence(Motion::Object, 160, 96, 4)));
    for (name, frames) in &seqs {
        let mut c: Vec<(f64, f64)> = (20..=240).step_by(20).map(|q| { let (b, p) = code_sequence_with(frames, q as u8, 6, set); (p, b as f64) }).collect();
        c.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for target in [30.0, 35.0, 40.0, 45.0] {
            if let Some(b) = curve_bytes_at(&c, target) {
                eprintln!("PT {name} {target} {b:.1}");
            }
        }
    }
}
