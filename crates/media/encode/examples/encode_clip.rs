// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Writes a synthetic clip: `cargo run --release --example encode_clip -- out.mp4 [w h frames kbps baseline|main cbr|vbr]`.
//! Also dumps the encoder's reconstruction as raw I420 next to it (`.yuv`).

use studio_encode::{aac, h264, mux, yuv::Yuv420};
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = a.get(1).cloned().unwrap_or_else(|| "clip.mp4".into());
    let w: u32 = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(1920);
    let h: u32 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(1080);
    let n: u32 = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(60);
    let kbps: u32 = a.get(5).and_then(|v| v.parse().ok()).unwrap_or(8000);
    let profile = if a.get(6).map(|s| s.as_str()) == Some("baseline") { h264::Profile::Baseline } else { h264::Profile::Main };
    let mode = if a.get(7).map(|s| s.as_str()) == Some("cbr") { h264::RateMode::Cbr } else { h264::RateMode::Vbr };
    let mut cfg = h264::Config::new(w, h, 30, 1);
    cfg.bitrate_kbps = kbps;
    cfg.profile = profile;
    cfg.rate_mode = mode;
    let mut venc = h264::Encoder::new(cfg).unwrap();
    venc.keep_reconstruction(true);
    let mut aenc = aac::Encoder::new(aac::Config { sample_rate: 48_000, channels: 2, bitrate: 192_000 }).unwrap();
    let opts = if path.ends_with(".mov") { mux::Options::mov() } else { mux::Options::mp4() };
    let mut m = mux::Muxer::create(&path, opts).unwrap();
    let vt = m.add_video(venc.track_info()).unwrap();
    let at = m.add_audio(aenc.track_info()).unwrap();
    let mut raw = std::io::BufWriter::new(std::fs::File::create(format!("{path}.yuv")).unwrap());
    let started = std::time::Instant::now();
    let mut bytes = 0usize;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for i in 0..n {
        for y in 0..h {
            for x in 0..w {
                let p = ((y * w + x) * 4) as usize;
                let cx = (x as f32 - w as f32 / 2.0 - (i as f32 * 7.0)) / 80.0;
                let cy = (y as f32 - h as f32 / 2.0) / 80.0;
                let ring = ((cx * cx + cy * cy).sqrt() * 3.0).sin() * 0.5 + 0.5;
                rgba[p] = (ring * 255.0) as u8;
                rgba[p + 1] = ((x + i * 4) % 256) as u8;
                rgba[p + 2] = ((y * 255) / h) as u8;
                rgba[p + 3] = 255;
            }
        }
        let pic = Yuv420::from_rgba(&rgba, w, h);
        let f = venc.encode(&pic);
        bytes += f.data.len();
        m.write_sample(vt, &f.data, f.keyframe).unwrap();
        let r = venc.reconstruction().unwrap();
        raw.write_all(&r.y).unwrap();
        raw.write_all(&r.u).unwrap();
        raw.write_all(&r.v).unwrap();
        let pcm: Vec<f32> = (0..1600 * 2).map(|k| ((i as usize * 1600 + k / 2) as f32 * 0.0575).sin() * 0.3).collect();
        for au in aenc.push(&pcm) {
            m.write_sample(at, &au, true).unwrap();
        }
    }
    for au in aenc.flush() {
        m.write_sample(at, &au, true).unwrap();
    }
    m.set_audio_samples(at, n as u64 * 1600);
    m.finish().unwrap();
    let secs = started.elapsed().as_secs_f64();
    println!("{n} frames {w}x{h} in {secs:.2}s ({:.1} fps), video {:.0} kbit/s", n as f64 / secs, bytes as f64 * 8.0 / (n as f64 / 30.0) / 1000.0);
}
