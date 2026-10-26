// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decode a stream (Annex-B .264/.h264/.jsv/.26l or MP4/MOV) and compare every output frame
//! with a reference I420 file. Prints the first mismatch and a per-frame summary.
//! Usage: h264cmp <stream> <reference.yuv>

#[path = "../tests/common/mp4mini.rs"]
mod mp4mini;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let reference = std::fs::read(&args[2]).expect("reference");
    let data = std::fs::read(path).expect("stream");
    let mut frames = Vec::new();
    let mut errors = Vec::new();
    let is_mp4 = data.len() > 8 && (&data[4..8] == b"ftyp" || &data[4..8] == b"moov" || &data[4..8] == b"wide" || &data[4..8] == b"mdat");
    let t0 = std::time::Instant::now();
    if !is_mp4 {
        let mut dec = studio_h264::Decoder::new_annex_b();
        // Feed access units roughly: whole stream at once is fine (boundaries are detected).
        match dec.decode(&data) {
            Ok(f) => frames.extend(f),
            Err(e) => errors.push(format!("{e}")),
        }
        frames.extend(dec.flush());
    } else {
        let track = mp4mini::read_video_track(&data).expect("track");
        let mut dec = studio_h264::Decoder::from_avcc_bytes(&track.avcc).expect("dec");
        for (i, s) in track.samples.iter().enumerate() {
            match dec.decode(s) {
                Ok(f) => frames.extend(f),
                Err(e) => errors.push(format!("sample {i}: {e}")),
            }
        }
        frames.extend(dec.flush());
    }
    let el = t0.elapsed();
    if frames.is_empty() {
        println!("FAIL {path}: no frames; errors: {errors:?}");
        std::process::exit(1);
    }
    let (w, h) = (frames[0].width as usize, frames[0].height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let fsize = w * h + 2 * cw * ch;
    let nref = reference.len() / fsize;
    let mut first = None;
    let mut bad_frames = Vec::new();
    for (i, f) in frames.iter().enumerate().take(nref) {
        let r = &reference[i * fsize..(i + 1) * fsize];
        let mut planes = vec![("Y", &f.y[..], &r[..w * h], w, 16usize)];
        planes.push(("U", &f.u[..], &r[w * h..w * h + cw * ch], cw, 8));
        planes.push(("V", &f.v[..], &r[w * h + cw * ch..], cw, 8));
        let mut bad = 0;
        for (pn, got, exp, pw, mbs) in planes {
            if got.len() != exp.len() {
                bad += 1;
                continue;
            }
            if let Some(p) = got.iter().zip(exp).position(|(a, b)| a != b) {
                bad += got.iter().zip(exp).filter(|(a, b)| a != b).count();
                if first.is_none() {
                    first = Some(format!(
                        "frame {i} poc {} plane {pn} x={} y={} mb=({},{}) got {} exp {}",
                        f.poc,
                        p % pw,
                        p / pw,
                        (p % pw) / mbs,
                        (p / pw) / mbs,
                        got[p],
                        exp[p]
                    ));
                }
            }
        }
        if bad > 0 {
            bad_frames.push((i, bad));
        }
    }
    let ok = first.is_none() && frames.len() == nref && errors.is_empty();
    println!(
        "{} {path}: {}x{} decoded {} ref {} bad frames {} errors {} ({:.0} fps)",
        if ok { "OK  " } else { "FAIL" },
        w,
        h,
        frames.len(),
        nref,
        bad_frames.len(),
        errors.len(),
        frames.len() as f64 / el.as_secs_f64()
    );
    if std::env::var("H264CMP_MAP").is_ok() {
        if let Some(&(fi, _)) = bad_frames.first() {
            let f = &frames[fi];
            let r = &reference[fi * fsize..fi * fsize + w * h];
            println!("   mismatch map of frame {fi} (luma, per 16x16; '.' ok, '#' bad):");
            for my in 0..h.div_ceil(16) {
                let mut line = String::new();
                for mx in 0..w.div_ceil(16) {
                    let mut bad = false;
                    for y in my * 16..(my * 16 + 16).min(h) {
                        for x in mx * 16..(mx * 16 + 16).min(w) {
                            if f.y[y * w + x] != r[y * w + x] {
                                bad = true;
                            }
                        }
                    }
                    line.push(if bad { '#' } else { '.' });
                }
                println!("   {line}");
            }
        }
    }
    if let Some(m) = first {
        println!("   first mismatch: {m}");
        println!("   bad frames: {:?}", &bad_frames[..bad_frames.len().min(20)]);
    }
    for e in errors.iter().take(5) {
        println!("   error: {e}");
    }
    if !ok {
        std::process::exit(1);
    }
}
