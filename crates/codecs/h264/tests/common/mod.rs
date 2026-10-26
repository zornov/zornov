// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(dead_code)]

pub mod mp4mini;
pub mod synth;
pub mod writer;

use std::path::PathBuf;

use studio_h264::{Decoder, Frame};

pub fn media_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/media")
}

/// Decode every sample of the fixture's video track; returns frames in output order.
pub fn decode_fixture(name: &str) -> (Vec<Frame>, mp4mini::Track) {
    let path = media_dir().join(name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let track = mp4mini::read_video_track(&data).expect("video track");
    let mut dec = Decoder::from_avcc_bytes(&track.avcc).expect("decoder");
    let mut frames = Vec::new();
    for (i, s) in track.samples.iter().enumerate() {
        match dec.decode_with_pts(s, Some(i as i64)) {
            Ok(f) => frames.extend(f),
            Err(e) => panic!("{name}: sample {i}: {e}"),
        }
    }
    frames.extend(dec.flush());
    (frames, track)
}

/// Compare decoded frames against the reference I420 dump; panics with the first mismatch.
pub fn compare_with_reference(name: &str, yuv: &str, width: usize, height: usize, expected_frames: usize) {
    let (frames, _track) = decode_fixture(name);
    let reference = read_yuv(&media_dir().join(yuv));
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    let fsize = width * height + 2 * cw * ch;
    assert_eq!(reference.len() % fsize, 0, "reference size");
    let nref = reference.len() / fsize;
    assert_eq!(nref, expected_frames, "reference frame count");
    let mut report = Vec::new();
    let mut first_bad: Option<String> = None;
    for (i, f) in frames.iter().enumerate().take(nref) {
        assert_eq!((f.width as usize, f.height as usize), (width, height), "frame {i} size");
        let r = &reference[i * fsize..(i + 1) * fsize];
        let planes: [(&str, &[u8], &[u8], usize); 3] = [
            ("Y", &f.y, &r[..width * height], width),
            ("U", &f.u, &r[width * height..width * height + cw * ch], cw),
            ("V", &f.v, &r[width * height + cw * ch..], cw),
        ];
        let mut bad = 0usize;
        for (pn, got, exp, w) in planes {
            if let Some(p) = got.iter().zip(exp.iter()).position(|(a, b)| a != b) {
                bad += got.iter().zip(exp.iter()).filter(|(a, b)| a != b).count();
                if first_bad.is_none() {
                    first_bad = Some(format!(
                        "{name}: first mismatch in frame {i} (poc {}), plane {pn}, x={} y={} (mb {},{}): got {} expected {}",
                        f.poc,
                        p % w,
                        p / w,
                        (p % w) / if pn == "Y" { 16 } else { 8 },
                        (p / w) / if pn == "Y" { 16 } else { 8 },
                        got[p],
                        exp[p]
                    ));
                }
            }
        }
        report.push(bad);
    }
    if first_bad.is_some() || frames.len() != nref {
        panic!(
            "{}\nmismatching samples per frame: {:?}\ndecoded {} frames, reference has {}",
            first_bad.unwrap_or_default(),
            report,
            frames.len(),
            nref
        );
    }
}

/// Reads a reference I420 dump kept gzipped in the repository (`name` + `.gz`).
fn read_yuv(path: &std::path::Path) -> Vec<u8> {
    use std::io::Read;
    let gz = std::fs::File::open(path.with_extension("yuv.gz")).expect("reference yuv.gz");
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz).read_to_end(&mut out).expect("gunzip reference yuv");
    out
}
