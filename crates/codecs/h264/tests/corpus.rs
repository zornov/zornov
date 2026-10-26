// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bit-exactness over a corpus of encoder-generated streams (tests/data, libx264 with many
//! tool combinations) and synthetic streams built by tests/common/synth.rs that exercise
//! tools encoders rarely emit. Golden hashes are FNV-1a 64 of the I420 output of an
//! independent reference decoder (ffmpeg) — see tests/data/golden.txt.

mod common;

use common::synth;
use studio_h264::{Decoder, Error, Frame, split_annex_b};

fn fnv(frames: &[Frame]) -> (u64, usize) {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut n = 0;
    for f in frames {
        for p in [&f.y, &f.u, &f.v] {
            for &b in p.iter() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100_0000_01b3);
            }
            n += p.len();
        }
    }
    (h, n)
}

fn data_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn stream(name: &str) -> Vec<u8> {
    if let Some(s) = name.strip_prefix("synth:") {
        synth::all().into_iter().find(|x| x.name == s).unwrap_or_else(|| panic!("no synthetic stream {s}")).data
    } else {
        std::fs::read(data_dir().join(name)).unwrap()
    }
}

/// Whole stream in one call.
fn decode_all(data: &[u8]) -> Result<Vec<Frame>, Error> {
    let mut d = Decoder::new_annex_b();
    let mut f = d.decode(data)?;
    f.extend(d.flush());
    Ok(f)
}

/// One NAL unit at a time through the streaming interface.
fn decode_streaming(data: &[u8]) -> Result<Vec<Frame>, Error> {
    let mut d = Decoder::new_annex_b();
    let mut f = Vec::new();
    for n in split_annex_b(data) {
        f.extend(d.push_nal(n)?);
    }
    f.extend(d.flush());
    Ok(f)
}

fn golden() -> Vec<(String, u64, usize)> {
    std::fs::read_to_string(data_dir().join("golden.txt"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let v: Vec<&str> = l.split_whitespace().collect();
            (v[0].to_string(), u64::from_str_radix(v[1], 16).unwrap(), v[2].parse().unwrap())
        })
        .collect()
}

fn check(prefix: &str) {
    let mut failures = Vec::new();
    let mut count = 0;
    for (name, hash, len) in golden() {
        if !name.starts_with(prefix) {
            continue;
        }
        count += 1;
        let data = stream(&name);
        for (mode, res) in [("whole", decode_all(&data)), ("streaming", decode_streaming(&data))] {
            match res {
                Ok(frames) => {
                    let (h, n) = fnv(&frames);
                    if (h, n) != (hash, len) {
                        failures.push(format!("{name} ({mode}): hash {h:016x}/{n} bytes, expected {hash:016x}/{len}"));
                    }
                }
                Err(e) => failures.push(format!("{name} ({mode}): {e}")),
            }
        }
    }
    assert!(count > 0, "no golden entries for {prefix}");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn encoder_corpus_bit_exact() {
    check("x264_");
}

#[test]
fn synthetic_streams_bit_exact() {
    check("synth:");
}

#[test]
fn fmo_aso_exact() {
    // ffmpeg does not implement FMO/ASO; the generator predicts the output exactly
    // (I_PCM and P_Skip only, deblocking disabled).
    let s = synth::fmo_aso();
    let frames = decode_all(&s.data).unwrap();
    let expected = s.expected.unwrap();
    let got: Vec<u8> = frames.iter().flat_map(|f| f.y.iter().chain(f.u.iter()).chain(f.v.iter()).copied()).collect();
    assert_eq!(got.len(), expected.len());
    if let Some(p) = got.iter().zip(&expected).position(|(a, b)| a != b) {
        let fsize = expected.len() / frames.len();
        panic!("mismatch in frame {} at byte {}", p / fsize, p % fsize);
    }
}

#[test]
fn unsupported_streams_are_rejected() {
    for name in ["unsupported_422.264", "unsupported_444.264", "unsupported_10bit.264", "unsupported_lossless.264"] {
        let path = data_dir().join(name);
        let Ok(data) = std::fs::read(&path) else { continue };
        match decode_all(&data) {
            Err(Error::Unsupported(msg)) => assert!(!msg.is_empty()),
            other => panic!("{name}: expected Unsupported, got {:?}", other.map(|f| f.len())),
        }
    }
}

#[test]
fn output_metadata() {
    let frames = decode_all(&stream("x264_vui_fullrange.264")).unwrap();
    let f = &frames[0];
    assert!(f.idr);
    assert!(f.color.full_range);
    assert_eq!(f.color.matrix_coefficients, 1);
    assert_eq!(f.sar, (16, 11));
    // Display order: POC increases between IDR pictures.
    for w in frames.windows(2) {
        assert!(w[1].idr || w[1].poc > w[0].poc, "poc {} then {}", w[0].poc, w[1].poc);
    }
    let rgba = studio_h264::to_rgba(f);
    assert_eq!(rgba.len(), (f.width * f.height * 4) as usize);
}

#[test]
fn data_partitioning_matches_unpartitioned() {
    // Re-package CAVLC streams (P/B slices with residuals, I_PCM, skips) into partitions
    // A/B/C; decoding must give exactly the same pictures.
    let names = [
        "x264_baseline_p4x4.264",
        "x264_high_cavlc_weightp.264",
        "x264_slice_max_cavlc.264",
        "x264_chroma_qp_offset.264",
        "synth:pcm_mix",
        "synth:deblock_slices",
        "synth:bipred_explicit",
    ];
    for name in names {
        let data = stream(name);
        let dp = studio_h264::testing::partition_stream(&data).unwrap();
        let types: Vec<u8> = split_annex_b(&dp).iter().map(|n| n[0] & 0x1f).collect();
        assert!(types.contains(&2), "{name}: no partitions produced");
        let a = decode_all(&data).unwrap();
        let b = decode_all(&dp).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(fnv(&a), fnv(&b), "{name}: partitioned decode differs");
        let c = decode_streaming(&dp).unwrap();
        assert_eq!(fnv(&a), fnv(&c), "{name}: partitioned streaming decode differs");
        // Losing partition C (inter residual) degrades the pictures (without constrained
        // intra prediction even intra nC contexts change) but every picture is still output.
        let mut dec = Decoder::new_annex_b();
        let mut n_out = 0;
        for n in split_annex_b(&dp) {
            if n[0] & 0x1f != 4 {
                n_out += dec.push_nal(n).map(|f| f.len()).unwrap_or(0);
            }
        }
        n_out += dec.flush().len();
        assert_eq!(n_out, a.len(), "{name}: frames lost without partition C");
    }
}

/// Writes every synthetic stream (`<name>.264`) and this decoder's I420 output
/// (`<name>.yuv`) to `$H264_DUMP`, to regenerate golden.txt against ffmpeg:
/// `ffmpeg -i x.264 -f rawvideo -pix_fmt yuv420p x.ref.yuv`.
#[test]
#[ignore]
fn dump_synthetic_streams() {
    let dir = std::path::PathBuf::from(std::env::var("H264_DUMP").expect("set H264_DUMP"));
    std::fs::create_dir_all(&dir).unwrap();
    for s in synth::all() {
        std::fs::write(dir.join(format!("{}.264", s.name)), &s.data).unwrap();
        let mut yuv = Vec::new();
        match decode_all(&s.data) {
            Ok(frames) => {
                for f in &frames {
                    for p in [&f.y, &f.u, &f.v] {
                        yuv.extend_from_slice(p);
                    }
                }
            }
            Err(e) => eprintln!("{}: {e}", s.name),
        }
        std::fs::write(dir.join(format!("{}.yuv", s.name)), &yuv).unwrap();
    }
}
