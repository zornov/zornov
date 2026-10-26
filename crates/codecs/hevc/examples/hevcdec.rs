// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Command-line decoder used for development and cross-checking:
//! `hevcdec <input.hevc|input.mp4|input.mov> <output.yuv>`
//! Writes I420 frames in display order (8-bit, or 16-bit little endian for
//! bit depths above 8).

#[path = "../tests/common/mp4mini.rs"]
mod mp4mini;

use studio_hevc::{Decoder, Frame, PlaneData};
use std::io::Write;

fn write_frame(out: &mut impl Write, f: &Frame) -> std::io::Result<()> {
    if std::env::var_os("HEVCDEC_PRINT_POC").is_some() {
        eprintln!("output poc {}", f.poc);
    }
    for p in [&f.y, &f.u, &f.v] {
        match p {
            PlaneData::U8(v) => out.write_all(v)?,
            PlaneData::U16(v) => {
                let mut b = Vec::with_capacity(v.len() * 2);
                for &s in v {
                    b.extend_from_slice(&s.to_le_bytes());
                }
                out.write_all(&b)?;
            }
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: hevcdec <input> <output.yuv>");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[1]).expect("read input");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&args[2]).expect("create output"));
    let t0 = std::time::Instant::now();
    let mut n = 0usize;
    let mut errors = 0usize;
    let is_mp4 = data.len() > 8 && (&data[4..8] == b"ftyp" || &data[4..8] == b"moov" || &data[4..8] == b"wide");
    if is_mp4 {
        let track = mp4mini::read_hevc_track(&data).expect("no HEVC track");
        let mut dec = Decoder::from_hvcc(&track.hvcc).expect("decoder");
        for (i, s) in track.samples.iter().enumerate() {
            match dec.decode(s) {
                Ok(fs) => {
                    for f in fs {
                        write_frame(&mut out, &f).unwrap();
                        n += 1;
                    }
                }
                Err(e) => {
                    errors += 1;
                    eprintln!("sample {i}: {e}");
                }
            }
        }
        for f in dec.flush() {
            write_frame(&mut out, &f).unwrap();
            n += 1;
        }
    } else {
        let mut dec = Decoder::new::<&[u8]>(&[], 4).unwrap();
        match dec.decode_annexb(&data) {
            Ok(fs) => {
                for f in fs {
                    write_frame(&mut out, &f).unwrap();
                    n += 1;
                }
            }
            Err(e) => {
                errors += 1;
                eprintln!("error: {e}");
            }
        }
        for f in dec.flush() {
            write_frame(&mut out, &f).unwrap();
            n += 1;
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    eprintln!("{n} frames, {errors} errors, {:.3}s ({:.1} fps)", secs, n as f64 / secs);
}
