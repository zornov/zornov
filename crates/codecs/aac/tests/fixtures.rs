// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decode the AAC fixtures and compare with Apple's decode (`.pcm`, f32le
//! interleaved, priming already trimmed by the edit list).

mod common;

use common::mp4mini::{read_audio_track, AudioTrack};
use studio_aac::Decoder;
use std::path::PathBuf;

fn media(name: &str) -> PathBuf {
    let local = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    if local.exists() {
        return local;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/media").join(name)
}

fn load(name: &str) -> (AudioTrack, Vec<f32>) {
    let file = std::fs::read(media(name)).expect("fixture");
    let track = read_audio_track(&file).expect("audio track");
    let pcm_name = name.rsplit_once('.').unwrap().0.to_string() + ".pcm";
    let raw = std::fs::read(media(&pcm_name)).expect("reference pcm");
    let pcm = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    (track, pcm)
}

struct Decoded {
    pcm: Vec<f32>,
    channels: usize,
    rate: u32,
    delay: usize,
    stats: studio_aac::DecoderStats,
}

fn decode(track: &AudioTrack) -> Decoded {
    let mut dec = Decoder::new(&track.asc).expect("decoder");
    let mut pcm = Vec::new();
    for (i, au) in track.samples.iter().enumerate() {
        let n = dec.decode_frame(au).unwrap_or_else(|e| panic!("frame {i}: {e}")).len();
        assert_eq!(n, dec.channels() * dec.samples_per_frame());
        pcm.extend_from_slice(dec.decode_frame_output());
    }
    Decoded { pcm, channels: dec.channels(), rate: dec.sample_rate(), delay: dec.output_delay(), stats: dec.stats().clone() }
}

/// Best integer lag (decoded index = reference index + lag) in -range..=range
/// by normalised cross-correlation of channel 0.
fn best_offset(dec: &[f32], refp: &[f32], ch: usize, range: i64) -> i64 {
    let n = refp.len() / ch;
    let m = dec.len() / ch;
    let win = n.min(24000);
    let start = (n / 8).min(4096);
    let mut best = (f64::MIN, 0i64);
    for lag in -range..=range {
        let mut acc = 0f64;
        let mut e = 0f64;
        for i in start..start + win.min(n - start) {
            let j = i as i64 + lag;
            if j < 0 || j as usize >= m {
                continue;
            }
            let d = dec[j as usize * ch] as f64;
            acc += d * refp[i * ch] as f64;
            e += d * d;
        }
        let score = if e > 0.0 { acc / e.sqrt() } else { f64::MIN };
        if score > best.0 {
            best = (score, lag);
        }
    }
    best.1
}

/// SNR in dB of `dec` (shifted by `lag`) against `refp` over their overlap.
fn snr(dec: &[f32], refp: &[f32], ch: usize, lag: i64) -> f64 {
    let n = refp.len() / ch;
    let m = dec.len() / ch;
    let (mut s, mut e) = (0f64, 0f64);
    for i in 0..n {
        let j = i as i64 + lag;
        if j < 0 || j as usize >= m {
            continue;
        }
        for c in 0..ch {
            let r = refp[i * ch + c] as f64;
            let d = dec[j as usize * ch + c] as f64;
            s += r * r;
            e += (r - d) * (r - d);
        }
    }
    10.0 * (s / e.max(1e-30)).log10()
}

fn check(name: &str, want_ch: usize, want_rate: u32, min_snr: f64) {
    let (track, refp) = load(name);
    let d = decode(&track);
    assert_eq!(d.channels, want_ch, "{name}: channels");
    assert_eq!(d.rate, want_rate, "{name}: sample rate");
    // Trim the encoder priming signalled by the edit list.
    // Without an edit list, assume Apple's standard 2112-sample core priming.
    let core_rate = studio_aac::AudioSpecificConfig::parse(&track.asc).unwrap().sample_rate as u64;
    let skip = if track.has_edit {
        (track.edit_media_time.max(0) as u64 * d.rate as u64 / track.timescale as u64) as usize
    } else {
        (2112 * d.rate as u64 / core_rate) as usize
    };
    let dec = &d.pcm[(skip * d.channels).min(d.pcm.len())..];
    let lag = best_offset(dec, &refp, d.channels, 4096);
    let s = snr(dec, &refp, d.channels, lag);
    println!(
        "{name}: {} ch @ {} Hz, {} AUs, edit skip {skip}, residual offset {lag} samples, SNR {s:.2} dB",
        d.channels,
        d.rate,
        track.samples.len()
    );
    println!("{name}: {:?}", d.stats);
    assert_eq!(lag, d.delay as i64, "{name}: alignment differs from Decoder::output_delay()");
    assert!(s >= min_snr, "{name}: SNR {s:.2} dB below {min_snr} dB (offset {lag})");
}

#[test]
fn lc_mono_22k() {
    check("aac_lc_mono_22k.m4a", 1, 22050, 60.0);
}

#[test]
fn lc_stereo_48k_baseline() {
    check("h264_baseline_cavlc.mp4", 2, 48000, 60.0);
}

#[test]
fn lc_mono_44k() {
    check("h264_main_cabac_bframes.mp4", 1, 44100, 60.0);
}

#[test]
fn lc_stereo_48k_high() {
    check("h264_high_cabac.mp4", 2, 48000, 60.0);
}

#[test]
fn he_aac_v1() {
    check("aac_he_v1.mp4", 2, 44100, 30.0);
}

#[test]
fn he_aac_v2() {
    check("aac_he_v2.mp4", 2, 44100, 30.0);
}

#[test]
fn extra_lc_48k_256k() {
    check("lc48_256.m4a", 2, 48000, 60.0);
}

#[test]
fn extra_lc_8k_mono() {
    check("lc8_mono.m4a", 1, 8000, 60.0);
}

#[test]
fn extra_he_48k_stereo() {
    check("he48.m4a", 2, 48000, 30.0);
}

#[test]
fn extra_he_32k_mono() {
    check("he32m.m4a", 1, 32000, 30.0);
}

#[test]
fn extra_he_22k_mono() {
    check("he22m.m4a", 1, 22050, 30.0);
}

#[test]
fn extra_he_v2_48k() {
    check("hev2_48.m4a", 2, 48000, 30.0);
}

#[test]
fn extra_he_v2_44k_40k() {
    check("hev2_44_40k.m4a", 2, 44100, 30.0);
}

#[test]
fn extra_kbd_windows() {
    check("ff_kbd44.m4a", 2, 44100, 60.0);
}

/// PNS bands carry random noise of a signalled energy: compare the decoded
/// band energies (per 1024 samples, 16 bands) with Apple's decode.
#[test]
fn pns_band_energies() {
    let (track, refp) = load("ff_pns44.m4a");
    let d = decode(&track);
    assert!(d.stats.noise_bands > 100, "stream exercises PNS");
    let skip = (track.edit_media_time.max(0) as u64 * d.rate as u64 / track.timescale as u64) as usize;
    let dec = &d.pcm[skip * 2..];
    let lag = best_offset(dec, &refp, 2, 4096);
    let n = 1024;
    let band_energy = |x: &[f32], start: usize, ch: usize| -> Vec<f64> {
        let mut e = vec![0f64; 16];
        for (b, eb) in e.iter_mut().enumerate() {
            for k in b * 32..(b + 1) * 32 {
                let (mut re, mut im) = (0f64, 0f64);
                for i in 0..n {
                    let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
                    let th = -2.0 * std::f64::consts::PI * (k * i) as f64 / n as f64;
                    let v = x[(start + i) * 2 + ch] as f64 * w;
                    re += v * th.cos();
                    im += v * th.sin();
                }
                *eb += re * re + im * im;
            }
        }
        e
    };
    let (mut sum, mut count, mut worst, mut bias) = (0f64, 0usize, 0f64, 0f64);
    for f in 2..(refp.len() / 2 / n - 2) {
        for ch in 0..2 {
            let a = band_energy(&refp, f * n, ch);
            let b = band_energy(dec, (f * n) as usize + lag as usize, ch);
            for (x, y) in a.iter().zip(&b) {
                if *x > 1e-6 {
                    let db = 10.0 * (y / x).log10();
                    sum += db.abs();
                    bias += db;
                    count += 1;
                    worst = f64::max(worst, db.abs());
                }
            }
        }
    }
    let mean = sum / count as f64;
    let bias = bias / count as f64;
    println!(
        "ff_pns44.m4a: offset {lag}, band energy deviation mean |{mean:.2}| dB, bias {bias:+.2} dB, worst {worst:.2} dB over {count} bands"
    );
    assert!(bias.abs() < 0.3, "PNS energy bias {bias:.2} dB");
    assert!(mean < 1.0, "PNS band energies deviate by {mean:.2} dB on average");
}

/// Split an ADTS stream into frames.
fn adts_frames(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 7 <= data.len() {
        let h = studio_aac::AdtsHeader::parse(&data[p..]).expect("adts header");
        out.push(&data[p..p + h.frame_length]);
        p += h.frame_length;
    }
    out
}

#[test]
fn adts_stream() {
    let data = std::fs::read(media("lc32.adts")).unwrap();
    let raw = std::fs::read(media("lc32.pcm")).unwrap();
    let refp: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let frames = adts_frames(&data);
    let mut dec = Decoder::new_adts(frames[0]).unwrap();
    assert_eq!((dec.channels(), dec.sample_rate()), (2, 32000));
    let mut pcm = Vec::new();
    for f in &frames {
        pcm.extend_from_slice(dec.decode_adts_frame(f).unwrap());
    }
    let lag = best_offset(&pcm, &refp, 2, 4096);
    let s = snr(&pcm, &refp, 2, lag);
    println!("lc32.adts: {} frames, offset {lag}, SNR {s:.2} dB", frames.len());
    assert!(s >= 60.0, "ADTS SNR {s:.2}");
}

const ALL: [&str; 15] = [
    "ff_kbd44.m4a",
    "ff_pns44.m4a",
    "aac_lc_mono_22k.m4a",
    "h264_baseline_cavlc.mp4",
    "h264_main_cabac_bframes.mp4",
    "h264_high_cabac.mp4",
    "aac_he_v1.mp4",
    "aac_he_v2.mp4",
    "lc48_256.m4a",
    "lc8_mono.m4a",
    "he48.m4a",
    "he32m.m4a",
    "he22m.m4a",
    "hev2_48.m4a",
    "hev2_44_40k.m4a",
];

/// Truncated, bit-flipped and random access units must produce `Err` or
/// output, never a panic; the decoder must stay usable afterwards.
#[test]
fn corrupt_frames_never_panic() {
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut errors = 0usize;
    let mut total = 0usize;
    for name in ALL {
        let file = std::fs::read(media(name)).unwrap();
        let track = read_audio_track(&file).unwrap();
        let mut dec = Decoder::new(&track.asc).unwrap();
        for au in &track.samples {
            // Truncations.
            for cut in [0, 1, 2, au.len() / 3, au.len() / 2, au.len().saturating_sub(1)] {
                total += 1;
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dec.decode_frame(&au[..cut]).is_err()));
                errors += r.unwrap_or_else(|_| panic!("{name}: panic on truncation to {cut}")) as usize;
            }
            // Bit flips.
            for _ in 0..8 {
                let mut bad = au.clone();
                let nflip = 1 + (rnd() % 6) as usize;
                for _ in 0..nflip {
                    let bit = (rnd() as usize) % (bad.len() * 8).max(1);
                    if !bad.is_empty() {
                        bad[bit / 8] ^= 0x80 >> (bit % 8);
                    }
                }
                total += 1;
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if let Ok(out) = dec.decode_frame(&bad) {
                        assert!(out.iter().all(|v| v.is_finite()), "non-finite output");
                        false
                    } else {
                        true
                    }
                }));
                errors += r.unwrap_or_else(|_| panic!("{name}: panic on bit flips")) as usize;
            }
            // Random garbage.
            let len = (rnd() % 600) as usize;
            let junk: Vec<u8> = (0..len).map(|_| rnd() as u8).collect();
            total += 1;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dec.decode_frame(&junk).is_err()));
            errors += r.unwrap_or_else(|_| panic!("{name}: panic on random data")) as usize;
            // The genuine frame decodes again. Delta-time coded SBR/PS data
            // may still reference corrupted history (an `Err` is correct
            // then); after a reset the decoder must be fully usable.
            if dec.decode_frame(au).is_err() {
                dec.reset();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dec.decode_frame(au).map(|o| o.len())));
                let _ = r.unwrap_or_else(|_| panic!("{name}: panic after reset"));
            }
        }
        // A fresh decoder still decodes the whole stream.
        let mut fresh = Decoder::new(&track.asc).unwrap();
        for au in &track.samples {
            fresh.decode_frame(au).unwrap();
        }
        // Broken configurations.
        for cut in 0..track.asc.len() {
            let _ = Decoder::new(&track.asc[..cut]);
        }
    }
    println!("corruption: {errors} of {total} damaged frames rejected with Err, none panicked");
    assert!(errors > total / 4);
}

#[test]
fn faster_than_50x_real_time() {
    let mut audio_seconds = 0f64;
    let mut elapsed = std::time::Duration::ZERO;
    for name in ALL {
        let file = std::fs::read(media(name)).unwrap();
        let track = read_audio_track(&file).unwrap();
        for _ in 0..5 {
            let mut dec = Decoder::new(&track.asc).unwrap();
            let t = std::time::Instant::now();
            let mut n = 0usize;
            for au in &track.samples {
                n += dec.decode_frame(au).unwrap().len();
            }
            elapsed += t.elapsed();
            audio_seconds += n as f64 / dec.channels() as f64 / dec.sample_rate() as f64;
        }
    }
    let speed = audio_seconds / elapsed.as_secs_f64();
    println!("decoded {audio_seconds:.1} s of audio in {:.3} s: {speed:.0}x real time", elapsed.as_secs_f64());
    if cfg!(debug_assertions) {
        return; // only meaningful in release builds
    }
    assert!(speed > 50.0, "{speed:.1}x real time");
}

#[test]
fn flush_and_reset() {
    let (track, _) = load("h264_high_cabac.mp4");
    let mut dec = Decoder::new(&track.asc).unwrap();
    for au in &track.samples[..10] {
        dec.decode_frame(au).unwrap();
    }
    let tail = dec.flush().to_vec();
    assert_eq!(tail.len(), 2048);
    assert!(tail.iter().any(|v| *v != 0.0), "flush returns the overlap tail");
    // After flush/reset the decoder restarts cleanly and is deterministic.
    let a: Vec<f32> = dec.decode_frame(&track.samples[0]).unwrap().to_vec();
    dec.reset();
    let b: Vec<f32> = dec.decode_frame(&track.samples[0]).unwrap().to_vec();
    assert_eq!(a, b);
}

/// Downsampled SBR (extension rate == core rate, 32-band synthesis): decode
/// the HE-AAC v1 fixture through an explicitly signalled downsampled config
/// and compare with Apple's full-rate decode decimated by two.
#[test]
fn downsampled_sbr() {
    let (track, refp) = load("aac_he_v1.mp4");
    // AOT 5, 22.05 kHz core, 2 channels, extension 22.05 kHz, AOT 2, GA 000.
    let mut bits = 0u32;
    for (v, w) in [(5u32, 5), (7, 4), (2, 4), (7, 4), (2, 5), (0, 3)] {
        bits = (bits << w) | v;
    }
    let asc = (bits << 7).to_be_bytes();
    let mut dec = Decoder::new(&asc).unwrap();
    assert!(dec.sbr_active());
    assert_eq!((dec.sample_rate(), dec.samples_per_frame(), dec.channels()), (22050, 1024, 2));
    let mut pcm = Vec::new();
    for au in &track.samples {
        pcm.extend_from_slice(dec.decode_frame(au).unwrap());
    }
    let dec_delay = dec.output_delay();
    assert!(pcm.iter().all(|v| v.is_finite()));
    // Half-band low-pass (windowed sinc) and decimate the reference.
    let taps: Vec<f64> = (-32i32..=32)
        .map(|n| {
            let x = n as f64 * 0.5;
            let sinc = if n == 0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
            0.5 * sinc * (0.54 + 0.46 * (std::f64::consts::PI * n as f64 / 33.0).cos())
        })
        .collect();
    let frames = refp.len() / 2;
    let mut dref = Vec::new();
    for i in (32..frames - 32).step_by(2) {
        for c in 0..2 {
            let mut acc = 0f64;
            for (t, h) in taps.iter().enumerate() {
                acc += h * refp[(i + t - 32) * 2 + c] as f64;
            }
            dref.push(acc as f32);
        }
    }
    let skip = 2112;
    let dec = &pcm[skip * 2..];
    let lag = best_offset(dec, &dref, 2, 1024);
    let s = snr(dec, &dref, 2, lag);
    println!("downsampled SBR: offset {lag}, SNR vs decimated full-rate reference {s:.2} dB");
    assert_eq!(lag, 16 + dec_delay as i64, "decimation filter delay + output_delay()");
    assert!(s > 50.0, "downsampled SBR SNR {s:.2}");
}

/// Implicit signalling: with only the 2-byte LC config, SBR (and PS) are
/// detected in the first access unit and the output switches to the doubled
/// rate (and stereo) before any samples are produced.
#[test]
fn implicit_sbr_and_ps_signalling() {
    for (name, ch) in [("aac_he_v1.mp4", 2), ("aac_he_v2.mp4", 2)] {
        let (track, _) = load(name);
        let mut explicit = Decoder::new(&track.asc).unwrap();
        let mut implicit = Decoder::new(&track.asc[..2]).unwrap();
        assert_eq!(implicit.sample_rate(), 22050);
        for au in &track.samples {
            let a = explicit.decode_frame(au).unwrap().to_vec();
            let b = implicit.decode_frame(au).unwrap().to_vec();
            assert_eq!((implicit.sample_rate(), implicit.channels()), (44100, ch), "{name}");
            assert_eq!(a, b, "{name}: implicit decode differs");
        }
        assert!(implicit.sbr_active());
        assert_eq!(implicit.ps_active(), name.contains("v2"));
    }
}
