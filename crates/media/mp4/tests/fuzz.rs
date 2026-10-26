// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Robustness: truncated and randomly mutated files must produce errors (or a
//! consistent movie), never panics or unbounded allocations.

mod common;

use std::io::Cursor;

use common::Rng;
use studio_mp4::{Demuxer, Error};

fn files() -> Vec<(String, Vec<u8>)> {
    common::index()
        .iter()
        .map(|e| {
            let name = e.str("file").unwrap().to_string();
            let data = std::fs::read(common::media_dir().join(&name)).unwrap();
            (name, data)
        })
        .collect()
}

/// Exercises a successfully parsed movie end to end.
fn exercise(data: Vec<u8>) -> Result<(), Error> {
    let len = data.len() as u64;
    let mut dmx = Demuxer::new(Cursor::new(data))?;
    let movie = dmx.movie().clone();
    for t in &movie.tracks {
        for s in &t.samples {
            assert!(s.offset + s.size as u64 <= len);
            dmx.read_sample(s)?;
            let _ = t.presentation_secs(s);
        }
        let _ = t.presentation_order();
        for secs in [0.0, 0.5, 1.0, 1e9, -1.0, f64::NAN] {
            if let Some(sp) = t.sample_for_time(secs) {
                assert!(sp.sync_index <= sp.target_index && sp.target_index < t.samples.len());
            }
        }
        if let Some(n) = t.codec.nal_length_size() {
            for s in t.samples.iter().take(3) {
                let _ = studio_mp4::split_nals(&dmx.read_sample(s)?, n);
            }
        }
    }
    Ok(())
}

/// Offset of the `moov` box, where mutations do the most damage.
fn moov_range(data: &[u8]) -> (usize, usize) {
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        if &data[pos + 4..pos + 8] == b"moov" {
            return (pos, pos + size);
        }
        pos += size.max(8);
    }
    (0, data.len())
}

#[test]
fn truncated_files_are_errors() {
    let mut rng = Rng::new(0x5eed);
    for (name, data) in files() {
        let mut cuts: Vec<usize> = vec![0, 1, 7, 8, 16, data.len() / 2, data.len() - 1];
        let (ms, me) = moov_range(&data);
        cuts.extend((0..40).map(|_| ms + rng.below(me - ms)));
        cuts.extend((0..20).map(|_| rng.below(data.len())));
        for cut in cuts {
            let r = Demuxer::new(Cursor::new(data[..cut].to_vec()));
            assert!(r.is_err(), "{name} truncated at {cut} parsed successfully");
        }
    }
}

#[test]
fn random_mutations_never_panic() {
    let mut rng = Rng::new(0xC0FFEE);
    // STUDIO_FUZZ_ITERS raises the per-file mutation count for longer runs.
    let iterations: usize =
        std::env::var("STUDIO_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let mut errors = 0;
    let mut runs = 0;
    for (_, data) in files() {
        let (ms, me) = moov_range(&data);
        for i in 0..iterations {
            let mut m = data.clone();
            let flips = 1 + rng.below(6);
            for _ in 0..flips {
                let at = if i % 4 == 0 { rng.below(m.len()) } else { ms + rng.below(me - ms) };
                match rng.below(3) {
                    0 => m[at] ^= 1 << rng.below(8),
                    1 => m[at] = rng.next() as u8,
                    _ => m[at] = [0x00, 0xff, 0x7f, 0x80][rng.below(4)],
                }
            }
            runs += 1;
            if exercise(m).is_err() {
                errors += 1;
            }
        }
    }
    // Many mutations land in fields that cannot be validated (timestamps,
    // matrices, payload bytes); the point is "no panic", but a fair share of
    // structural damage must still be detected.
    assert!(errors * 10 > runs, "only {errors}/{runs} mutations rejected");
}

#[test]
fn garbage_inputs() {
    let mut rng = Rng::new(42);
    for n in [0usize, 1, 8, 64, 4096] {
        let junk: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        assert!(Demuxer::new(Cursor::new(junk)).is_err());
    }
    // A box claiming a 2^64-1 size and a moov claiming a huge payload.
    let mut huge = vec![0, 0, 0, 1, b'm', b'o', b'o', b'v'];
    huge.extend_from_slice(&u64::MAX.to_be_bytes());
    assert!(Demuxer::new(Cursor::new(huge)).is_err());
    let lying = [0xff, 0xff, 0xff, 0xff, b'm', b'o', b'o', b'v', 0, 0, 0, 0];
    assert!(Demuxer::new(Cursor::new(lying.to_vec())).is_err());
}
