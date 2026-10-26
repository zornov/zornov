// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! AAC-LC round trips through the workspace's hand-written AAC decoder.

use studio_encode::aac::{Config, Encoder, PRIMING};

fn signal(rate: u32, channels: usize, secs: f64) -> Vec<f32> {
    let n = (rate as f64 * secs) as usize;
    let mut v = Vec::with_capacity(n * channels);
    for i in 0..n {
        let t = i as f64 / rate as f64;
        for c in 0..channels {
            let f = if c == 0 { 1000.0 } else { 440.0 };
            let s = 0.5 * (2.0 * std::f64::consts::PI * f * t).sin() + 0.1 * (2.0 * std::f64::consts::PI * 3150.0 * t).sin();
            v.push(s as f32);
        }
    }
    v
}

/// Encode, decode, drop the priming and return (input, output, access units).
fn run(cfg: Config, secs: f64) -> (Vec<f32>, Vec<f32>, usize, usize) {
    let ch = cfg.channels as usize;
    let input = signal(cfg.sample_rate, ch, secs);
    let mut enc = Encoder::new(cfg).unwrap();
    let mut aus = Vec::new();
    for chunk in input.chunks(777 * ch) {
        aus.extend(enc.push(chunk));
    }
    aus.extend(enc.flush());
    assert_eq!(aus.len() as u64, Encoder::frames_for((input.len() / ch) as u64));
    let mut dec = studio_aac::Decoder::new(&enc.audio_specific_config()).expect("ASC accepted");
    assert_eq!(dec.sample_rate(), cfg.sample_rate);
    assert_eq!(dec.channels(), ch);
    let mut out = Vec::new();
    let mut bytes = 0;
    for au in &aus {
        bytes += au.len();
        out.extend_from_slice(dec.decode_frame(au).expect("decodes"));
    }
    let skip = PRIMING as usize * ch;
    let out = out[skip..skip + input.len()].to_vec();
    (input, out, aus.len(), bytes)
}

fn snr_db(a: &[f32], b: &[f32]) -> f64 {
    let sig: f64 = a.iter().map(|&x| (x as f64).powi(2)).sum();
    let err: f64 = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum();
    10.0 * (sig / err.max(1e-20)).log10()
}

#[test]
fn stereo_48k_sine_round_trip() {
    let cfg = Config { sample_rate: 48_000, channels: 2, bitrate: 192_000 };
    let (input, out, frames, bytes) = run(cfg, 1.0);
    let snr = snr_db(&input, &out);
    let kbps = bytes as f64 * 8.0 / (frames as f64 * 1024.0 / 48_000.0) / 1000.0;
    eprintln!("48k stereo: SNR {snr:.1} dB, {kbps:.0} kbit/s");
    assert!(snr > 25.0, "SNR {snr:.1} dB");
    assert!(kbps < 192.0 * 1.15, "{kbps:.0} kbit/s");
    // Amplitude is preserved (no scale error).
    let peak_in = input.iter().fold(0f32, |m, v| m.max(v.abs()));
    let peak_out = out.iter().fold(0f32, |m, v| m.max(v.abs()));
    assert!((peak_out / peak_in - 1.0).abs() < 0.05, "{peak_in} vs {peak_out}");
}

#[test]
fn mono_44k_round_trip() {
    let cfg = Config { sample_rate: 44_100, channels: 1, bitrate: 96_000 };
    let (input, out, _, _) = run(cfg, 0.5);
    let snr = snr_db(&input, &out);
    eprintln!("44.1k mono: SNR {snr:.1} dB");
    assert!(snr > 25.0, "SNR {snr:.1} dB");
}

#[test]
fn low_bitrate_still_decodes() {
    let cfg = Config { sample_rate: 48_000, channels: 2, bitrate: 32_000 };
    let (input, out, frames, bytes) = run(cfg, 0.5);
    let kbps = bytes as f64 * 8.0 / (frames as f64 * 1024.0 / 48_000.0) / 1000.0;
    assert!(kbps < 32.0 * 1.2, "{kbps:.0} kbit/s");
    assert!(snr_db(&input, &out) > 6.0);
}

#[test]
fn noise_and_silence_round_trip() {
    let cfg = Config { sample_rate: 48_000, channels: 2, bitrate: 128_000 };
    let mut enc = Encoder::new(cfg).unwrap();
    let mut seed = 1u32;
    let mut input = Vec::new();
    for i in 0..48_000 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let x = if i < 24_000 { ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * 1.9 } else { 0.0 };
        input.push(x);
        input.push(-x);
    }
    let mut aus = enc.push(&input);
    aus.extend(enc.flush());
    let mut dec = studio_aac::Decoder::new(&enc.audio_specific_config()).unwrap();
    for au in &aus {
        assert!(au.len() * 8 <= 6144 * 2, "access unit too large: {} bytes", au.len());
        dec.decode_frame(au).unwrap();
    }
}

#[test]
fn surround_51_round_trip_keeps_each_channel_in_place() {
    // SMPTE input order L R C LFE Ls Rs, one tone per channel.
    let rate = 48_000u32;
    let freqs = [500.0, 700.0, 1100.0, 60.0, 1700.0, 2300.0];
    let n = rate as usize;
    let mut input = Vec::with_capacity(n * 6);
    for i in 0..n {
        let t = i as f64 / rate as f64;
        for f in freqs {
            input.push((0.3 * (std::f64::consts::TAU * f * t).sin()) as f32);
        }
    }
    let mut enc = Encoder::new(Config { sample_rate: rate, channels: 6, bitrate: 384_000 }).unwrap();
    assert_eq!(enc.audio_specific_config(), vec![0x11, 0xB0], "AAC-LC, 48 kHz, channel configuration 6");
    let mut aus = Vec::new();
    for chunk in input.chunks(1000 * 6) {
        aus.extend(enc.push(chunk));
    }
    aus.extend(enc.flush());
    let mut dec = studio_aac::Decoder::new(&enc.audio_specific_config()).expect("ASC accepted");
    assert_eq!(dec.channels(), 6);
    let mut out = Vec::new();
    for au in &aus {
        out.extend_from_slice(dec.decode_frame(au).expect("decodes"));
    }
    let out = &out[PRIMING as usize * 6..];
    // Decoder order is AAC's: C L R Ls Rs LFE.
    let aac_to_smpte = [2usize, 0, 1, 4, 5, 3];
    for (k, &src) in aac_to_smpte.iter().enumerate() {
        let a: Vec<f32> = input.iter().skip(src).step_by(6).copied().take(n - 4096).collect();
        let b: Vec<f32> = out.iter().skip(k).step_by(6).copied().take(n - 4096).collect();
        let snr = snr_db(&a, &b);
        eprintln!("channel {k} (input {src}): SNR {snr:.1} dB");
        assert!(snr > 15.0, "channel {k} (input {src}) SNR {snr:.1} dB");
    }
    let kbps = aus.iter().map(Vec::len).sum::<usize>() as f64 * 8.0 / (aus.len() as f64 * 1024.0 / rate as f64) / 1000.0;
    assert!(kbps < 384.0 * 1.15, "{kbps:.0} kbit/s");
}
