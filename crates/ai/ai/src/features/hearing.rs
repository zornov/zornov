// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hearing: what a sound *is*, not only how loud. A local, model-free ear
//! for the AI editor.
//!
//! * [`analyse`] tags a recording, half a second at a time, as silence,
//!   speech, music, speech over music, applause or crowd, or steady noise,
//!   and finds the faults an editor listens for: clipping, clicks and
//!   pops, mains hum (50 or 60 Hz and its harmonics), a rumble; and it
//!   describes a voice (its pitch, how fast it talks) and the room (dry
//!   and close, or roomy).
//! * [`balance`] compares a voice with the music under it: how far below
//!   the voice the music sits while someone speaks, and where the music
//!   masks the voice in the bands that carry the consonants (1–4 kHz).
//! * [`describe`] says all of it in a few plain sentences.
//!
//! The features are the usual ones (Scheirer & Slaney 1997, "Construction
//! and evaluation of a robust multifeature speech/music discriminator"):
//! 4 Hz modulation of the speech-band energy (syllables), voiced frames
//! and how pitch moves, the persistence of spectral peaks (held notes),
//! spectral flatness and onset density (applause, noise).

use studio_nn::audio::Fft;

/// Working rate: enough for speech and music features.
const RATE: u32 = 16_000;
const FRAME: usize = 512;
const HOP: usize = 250;
/// Seconds per analysed frame hop.
const HOP_S: f64 = HOP as f64 / RATE as f64;
/// One label every half second.
const WINDOW_S: f64 = 0.5;

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-12).log10()
}

/// `x` (at `rate`) at 16 kHz: box-filtered and linearly interpolated (the
/// features above 8 kHz are not used).
pub fn to_16k(x: &[f32], rate: u32) -> Vec<f32> {
    if rate == RATE {
        return x.to_vec();
    }
    let k = rate as f64 / RATE as f64;
    let n = (x.len() as f64 / k).floor() as usize;
    let half = (k / 2.0).max(0.5);
    (0..n)
        .map(|i| {
            let c = i as f64 * k;
            let (a, b) = ((c - half).max(0.0) as usize, ((c + half).ceil() as usize).min(x.len()).max(1));
            let s: f32 = x[a.min(b - 1)..b].iter().sum();
            s / (b - a.min(b - 1)) as f32
        })
        .collect()
}

/// Per-frame features.
#[derive(Clone, Debug, Default)]
struct Feat {
    rms_db: f64,
    /// Band energies (dB): <150, 150–500, 500–1k, 1–2k, 2–4k, 4–8k Hz.
    bands: [f64; 6],
    flatness: f64,
    centroid: f64,
    flux: f64,
    /// Below 70 Hz (rumble), dB.
    sub_db: f64,
    /// Fundamental (Hz) when voiced.
    f0: Option<f64>,
    /// Spectral peak bins (100 Hz–4 kHz).
    peaks: Vec<usize>,
}

fn frames(x: &[f32]) -> Vec<Feat> {
    if x.len() < FRAME {
        return Vec::new();
    }
    let fft = Fft::new(FRAME);
    let fft2 = Fft::new(FRAME * 2);
    let win: Vec<f32> = (0..FRAME).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos()).collect();
    let bin_hz = RATE as f64 / FRAME as f64;
    let edges = [0.0, 150.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
    let mut out = Vec::with_capacity(x.len() / HOP);
    let mut prev: Option<Vec<f64>> = None;
    let mut i = 0;
    while i + FRAME <= x.len() {
        let f = &x[i..i + FRAME];
        let rms = (f.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / FRAME as f64).sqrt();
        let w: Vec<f32> = f.iter().zip(&win).map(|(a, b)| a * b).collect();
        let (re, im) = fft.rfft(&w);
        let p: Vec<f64> = re.iter().zip(&im).map(|(a, b)| (*a as f64).powi(2) + (*b as f64).powi(2)).collect();
        let mut bands = [0.0; 6];
        for (k, v) in p.iter().enumerate() {
            let hz = k as f64 * bin_hz;
            if let Some(b) = (0..6).find(|&b| hz >= edges[b] && hz < edges[b + 1]) {
                bands[b] += v;
            }
        }
        let bands = bands.map(|b| db(b / FRAME as f64));
        let (lo, hi) = ((100.0 / bin_hz) as usize, (6000.0 / bin_hz) as usize);
        let seg = &p[lo..hi.min(p.len())];
        let am = seg.iter().sum::<f64>() / seg.len() as f64;
        let gm = (seg.iter().map(|v| (v + 1e-12).ln()).sum::<f64>() / seg.len() as f64).exp();
        let flatness = if am > 1e-12 { (gm / am).clamp(0.0, 1.0) } else { 0.0 };
        let tot: f64 = p.iter().sum();
        let centroid = if tot > 1e-12 { p.iter().enumerate().map(|(k, v)| k as f64 * bin_hz * v).sum::<f64>() / tot } else { 0.0 };
        let mag: Vec<f64> = p.iter().map(|v| v.sqrt()).collect();
        let flux = match &prev {
            Some(q) => {
                let s: f64 = mag.iter().zip(q).map(|(a, b)| (a - b).max(0.0)).sum();
                s / mag.iter().sum::<f64>().max(1e-9)
            }
            None => 0.0,
        };
        // Peaks: local maxima well above the frame's level, 100 Hz–4 kHz.
        let pmax = p[lo..(4000.0 / bin_hz) as usize].iter().cloned().fold(0.0, f64::max);
        let peaks: Vec<usize> = (lo.max(1)..(4000.0 / bin_hz) as usize).filter(|&k| p[k] > p[k - 1] && p[k] >= p[k + 1] && p[k] > pmax * 0.05).collect();
        // A voice's harmonics carry most of its voiced energy below 1 kHz;
        // a cricket's 4-5 kHz tone, a hiss or a cymbal have no pitch here.
        let voice_band = p.iter().enumerate().filter(|(k, _)| (80.0..1000.0).contains(&(*k as f64 * bin_hz))).map(|(_, v)| v).sum::<f64>();
        let f0 = if db(rms * rms) > -50.0 && voice_band > 0.05 * p.iter().sum::<f64>() { pitch(f, &fft2) } else { None };
        let sub_db = db((p[1] + p[2]) / FRAME as f64);
        out.push(Feat { rms_db: db(rms * rms), bands, sub_db, flatness, centroid, flux, f0, peaks });
        prev = Some(mag);
        i += HOP;
    }
    out
}

/// Autocorrelation pitch (70–400 Hz) through a zero-padded FFT, `None`
/// when not voiced: the normalised cross-correlation at the best lag (each
/// lag against the energy of the two stretches it compares, so long lags
/// are not favoured) under 0.6.
fn pitch(f: &[f32], fft: &Fft) -> Option<f64> {
    let (lo, hi) = ((RATE / 400) as usize, (RATE / 70) as usize);
    let n = f.len();
    let mean = f.iter().sum::<f32>() / n as f32;
    let raw: Vec<f32> = f.iter().map(|v| v - mean).collect();
    // The voice band alone (60 Hz - 1 kHz): a bright bed (crickets, hiss)
    // over a voice neither hides its pitch nor lends it one.
    let (mut re, mut im) = fft.rfft(&raw);
    let bin = RATE as f64 / (2 * n) as f64;
    for k in 0..re.len() {
        let hz = k as f64 * bin;
        if !(60.0..=1000.0).contains(&hz) {
            re[k] = 0.0;
            im[k] = 0.0;
        }
    }
    let x: Vec<f32> = fft.irfft(&re, &im)[..n].to_vec();
    let (re, im) = fft.rfft(&x);
    let pw: Vec<f32> = re.iter().zip(&im).map(|(a, b)| a * a + b * b).collect();
    let r = fft.irfft(&pw, &vec![0.0; pw.len()]);
    if r[0] <= 1e-9 {
        return None;
    }
    // Energy of the first n − lag and the last n − lag samples.
    let mut pre = vec![0f64; n + 1];
    for (i, v) in x.iter().enumerate() {
        pre[i + 1] = pre[i] + (*v as f64) * (*v as f64);
    }
    let nccf = |lag: usize| -> f32 {
        let (e0, e1) = (pre[n - lag], pre[n] - pre[lag]);
        if e0 * e1 > 1e-18 { (r[lag] as f64 / (e0 * e1).sqrt()) as f32 } else { 0.0 }
    };
    let top = hi.min(n - 2);
    let mut best = (0.0f32, 0usize);
    for lag in lo..=top {
        let v = nccf(lag);
        if v > best.0 {
            best = (v, lag);
        }
    }
    if best.0 <= 0.6 {
        return None;
    }
    // The peak between lags, by a parabola through its neighbours, so the
    // pitch moves smoothly rather than in whole-sample steps.
    let l = best.1;
    let (a, b, c) = (nccf(l - 1) as f64, best.0 as f64, nccf(l + 1) as f64);
    let d = a - 2.0 * b + c;
    let shift = if l > lo && l < top && d.abs() > 1e-9 { (0.5 * (a - c) / d).clamp(-0.5, 0.5) } else { 0.0 };
    Some(RATE as f64 / (l as f64 + shift))
}

/// Rate the hum search works at: harmonics up to 360 Hz need no more.
const HUM_RATE: usize = 2_000;

/// The DFT of `x` (at [`HUM_RATE`], already windowed) at `hz`.
fn dft_at(x: &[f64], hz: f64) -> (f64, f64) {
    let w = 2.0 * std::f64::consts::PI * hz / HUM_RATE as f64;
    let (c, s) = (w.cos(), w.sin());
    let (mut re, mut im) = (1.0f64, 0.0f64);
    let (mut acc_re, mut acc_im) = (0.0, 0.0);
    for &v in x {
        acc_re += v * re;
        acc_im -= v * im;
        let r = re * c - im * s;
        im = re * s + im * c;
        re = r;
    }
    (acc_re, acc_im)
}

/// Power of `x` at `hz`.
fn power_at(x: &[f64], hz: f64) -> f64 {
    let (re, im) = dft_at(x, hz);
    (re * re + im * im).max(1e-30)
}

/// Mains hum: a comb at 50 or 60 Hz and its harmonics, steady through the
/// take. Mains is locked to the grid and never stops, so the whole take
/// (up to 30 s) is analysed as one Hann-windowed tone search, which
/// resolves frequencies a few hundredths of a hertz apart: each harmonic's
/// strongest frequency within ±1 Hz must sit on the mains frequency (within
/// what grid drift allows) and stand 10 dB over its neighbours half a hertz
/// to three hertz away. A bass note is off the grid (58 Hz is a B flat, not
/// 60), and the sidebands a rhythm puts around a note sit a whole number of
/// beats-per-second away, so neither lands on the comb. Hum is two or more
/// harmonics on the comb, or a fundamental 15 dB clear. Returns the mains
/// frequency and how far its strongest harmonic stands out (dB).
fn mains_hum(x: &[f32]) -> Option<(f64, f64)> {
    // Down to 2 kHz through a 500 Hz low-pass (4th-order Butterworth).
    let step = RATE as usize / HUM_RATE;
    let lp = |x: &[f64], q: f64| -> Vec<f64> {
        let w = 2.0 * std::f64::consts::PI * 500.0 / RATE as f64;
        let (sn, cs) = w.sin_cos();
        let alpha = sn / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1, b2, a1, a2) = ((1.0 - cs) / 2.0 / a0, (1.0 - cs) / a0, (1.0 - cs) / 2.0 / a0, -2.0 * cs / a0, (1.0 - alpha) / a0);
        let (mut z1, mut z2) = (0.0, 0.0);
        x.iter()
            .map(|&v| {
                let y = b0 * v + z1;
                z1 = b1 * v - a1 * y + z2;
                z2 = b2 * v - a2 * y;
                y
            })
            .collect()
    };
    let wide: Vec<f64> = x.iter().map(|&v| v as f64).collect();
    let low = lp(&lp(&wide, 0.541_196_1), 1.306_563);
    let y: Vec<f64> = low.iter().step_by(step).copied().collect();
    // Two seconds at least: shorter takes cannot tell 58 Hz from 60.
    if y.len() < 2 * HUM_RATE || y.iter().map(|v| v * v).sum::<f64>() < 1e-9 {
        return None;
    }
    let n = y.len();
    let w: Vec<f64> = y.iter().enumerate().map(|(i, v)| v * (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())).collect();
    // Frequency resolution of the window: its main lobe is ±2/T.
    let lobe = 2.0 * HUM_RATE as f64 / n as f64;
    let mut best: Option<(f64, f64, f64)> = None;
    for base in [50.0, 60.0] {
        let mut on = Vec::new();
        for k in 1..=6 {
            let f = base * k as f64;
            let fine = (lobe / 4.0).min(0.05);
            let steps = (1.0 / fine).ceil() as i32;
            let (peak_hz, peak) = (-steps..=steps).map(|s| f + s as f64 * fine).map(|g| (g, power_at(&w, g))).fold((f, 0.0), |a, c| if c.1 > a.1 { c } else { a });
            let mut around: Vec<f64> = [0.5f64, 0.8, 1.2, 2.0, 3.0].iter().map(|d| d.max(1.5 * lobe)).flat_map(|d| [f - d, f + d]).map(|g| power_at(&w, g)).collect();
            around.sort_by(f64::total_cmp);
            // The median neighbour: music beside the comb does not hide it.
            let prom = db(peak / around[around.len() / 2]);
            // Grid drift moves harmonic k k times as far.
            let drift = 0.04 * k as f64 + lobe / 4.0;
            if (peak_hz - f).abs() <= drift && prom > 10.0 {
                on.push((k, prom));
            }
        }
        let fundamental = on.iter().find(|h| h.0 == 1).map(|h| h.1);
        if on.len() >= 2 || fundamental.is_some_and(|p| p > 15.0) {
            let strongest = on.iter().map(|h| h.1).fold(0.0, f64::max);
            let score = on.iter().map(|h| h.1).sum::<f64>();
            if best.is_none_or(|(_, _, s)| score > s) {
                best = Some((base, (strongest.min(60.0) * 10.0).round() / 10.0, score));
            }
        }
    }
    best.map(|(base, prom, _)| (base, prom))
}

/// One labelled stretch.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub from: f64,
    pub to: f64,
    /// silence, speech, music, speech over music, applause, noise, sound.
    pub label: &'static str,
}

/// What [`analyse`] heard.
#[derive(Clone, Debug, Default)]
pub struct Heard {
    pub duration: f64,
    pub segments: Vec<Segment>,
    /// Seconds of each label.
    pub shares: Vec<(&'static str, f64)>,
    /// Moments of clipping (seconds), and how many clipped runs.
    pub clipping: Vec<f64>,
    pub clipped_runs: usize,
    /// Clicks and pops (seconds).
    pub clicks: Vec<f64>,
    /// Mains hum: frequency and how far it stands out (dB).
    pub hum: Option<(f64, f64)>,
    /// Energy below 80 Hz against the rest while sounding (dB).
    pub rumble_db: f64,
    /// A voice's median pitch (Hz) and its spread (semitones).
    pub voice_pitch: Option<(f64, f64)>,
    /// Syllables a second while talking (about 4–5 is conversational).
    pub syllable_rate: Option<f64>,
    /// How roomy speech sounds: 0 dry and close, 1 echoey.
    pub room: Option<f64>,
    /// Spectral tilt: energy above 4 kHz against 500 Hz–2 kHz (dB).
    pub air_db: f64,
}

/// Listens to `mono` at `rate`.
pub fn analyse(mono: &[f32], rate: u32) -> Heard {
    let duration = mono.len() as f64 / rate.max(1) as f64;
    let mut h = Heard { duration, ..Heard::default() };
    // Faults at the full rate.
    let mut run = 0usize;
    let mut last_clip = -1.0;
    for (i, &v) in mono.iter().enumerate() {
        if v.abs() >= 0.999 {
            run += 1;
            if run == 3 {
                h.clipped_runs += 1;
                let t = i as f64 / rate as f64;
                if t - last_clip > 0.25 {
                    h.clipping.push((t * 100.0).round() / 100.0);
                    last_clip = t;
                }
            }
        } else {
            run = 0;
        }
    }
    h.clicks = clicks(mono, rate);
    let x = to_16k(mono, rate);
    let fs = frames(&x);
    if fs.is_empty() {
        return h;
    }
    h.hum = mains_hum(&x[..x.len().min(RATE as usize * 30)]);
    // The speech-band envelope, for syllable modulation.
    let env: Vec<f64> = fs.iter().map(|f| {
        let e = 10f64.powf(f.bands[1] / 10.0) + 10f64.powf(f.bands[2] / 10.0) + 10f64.powf(f.bands[3] / 10.0) + 10f64.powf(f.bands[4] / 10.0);
        db(e)
    }).collect();
    let per_win = (WINDOW_S / HOP_S).round() as usize;
    let nwin = fs.len().div_ceil(per_win);
    let ctx = (2.0 / HOP_S) as usize; // 2 s of context
    let mut labels: Vec<&'static str> = Vec::with_capacity(nwin);
    for w in 0..nwin {
        let (a, b) = (w * per_win, ((w + 1) * per_win).min(fs.len()));
        let win = &fs[a..b];
        let loud = win.iter().map(|f| f.rms_db).fold(f64::NEG_INFINITY, f64::max);
        if loud < -50.0 {
            labels.push("silence");
            continue;
        }
        let c0 = ((a + b) / 2).saturating_sub(ctx / 2).min(env.len().saturating_sub(ctx));
        let c1 = (c0 + ctx).min(env.len());
        let wf = window_feats(win, &env[c0..c1], &fs[c0..c1], loud);
        let label = classify(&wf);
        labels.push(label);
    }
    // Smooth single odd windows (a lone "music" half second inside speech).
    for i in 1..labels.len().saturating_sub(1) {
        if labels[i - 1] == labels[i + 1] && labels[i] != labels[i - 1] && labels[i] != "silence" && labels[i - 1] != "silence" {
            labels[i] = labels[i - 1];
        }
    }
    for (i, l) in labels.iter().enumerate() {
        let (from, to) = (i as f64 * WINDOW_S, ((i + 1) as f64 * WINDOW_S).min(duration));
        match h.segments.last_mut() {
            Some(s) if s.label == *l => s.to = to,
            _ => h.segments.push(Segment { from, to, label: l }),
        }
        match h.shares.iter_mut().find(|s| s.0 == *l) {
            Some(s) => s.1 += to - from,
            None => h.shares.push((l, to - from)),
        }
    }
    h.shares.sort_by(|a, b| b.1.total_cmp(&a.1));
    // A spike inside music is a hi-hat or a snare, not a fault.
    h.clicks.retain(|&t| !matches!(labels.get((t / WINDOW_S) as usize).copied(), Some("music" | "speech over music")));
    // Voice: on speech windows.
    let speech_frames: Vec<&Feat> = labels.iter().enumerate().filter(|(_, l)| l.starts_with("speech")).flat_map(|(w, _)| fs[w * per_win..((w + 1) * per_win).min(fs.len())].iter()).collect();
    let mut v0: Vec<f64> = speech_frames.iter().filter_map(|f| f.f0).collect();
    if v0.len() > 20 {
        v0.sort_by(f64::total_cmp);
        let med = v0[v0.len() / 2];
        let st: Vec<f64> = v0.iter().map(|f| 12.0 * (f / med).log2()).collect();
        let spread = (st.iter().map(|s| s * s).sum::<f64>() / st.len() as f64).sqrt();
        h.voice_pitch = Some(((med * 10.0).round() / 10.0, (spread * 10.0).round() / 10.0));
        // Syllables: peaks of the speech-band envelope while talking.
        let talk: Vec<usize> = labels.iter().enumerate().filter(|(_, l)| **l == "speech").map(|(w, _)| w).collect();
        let mut peaks = 0usize;
        let mut secs = 0.0;
        for &w in &talk {
            let (a, b) = (w * per_win, ((w + 1) * per_win).min(env.len()));
            let e = &env[a..b];
            let mean = e.iter().sum::<f64>() / e.len().max(1) as f64;
            let mut last = -10i64;
            for i in 2..e.len().saturating_sub(2) {
                if e[i] > e[i - 1] && e[i] >= e[i + 1] && e[i] > e[i - 2] && e[i] >= e[i + 2] && e[i] > mean + 3.0 && (i as i64 - last) >= 6 {
                    peaks += 1;
                    last = i as i64;
                }
            }
            secs += (b - a) as f64 * HOP_S;
        }
        if secs > 1.0 {
            h.syllable_rate = Some((peaks as f64 / secs * 10.0).round() / 10.0);
        }
        // Room: how slowly speech energy falls after its peaks (decay over
        // the 120 ms after a syllable, against a dry voice's ~40 dB/s).
        let mut slopes = Vec::new();
        for &w in &talk {
            let (a, b) = (w * per_win, ((w + 1) * per_win).min(env.len()));
            for i in a + 1..b.saturating_sub(8) {
                // Phrase ends that fall to near silence (a bed under the
                // voice would read as a room).
                let floor = env[i..(i + 20).min(env.len())].iter().cloned().fold(f64::INFINITY, f64::min);
                if env[i] > env[i - 1] && env[i] >= env[i + 1] && env[i + 7] < env[i] - 6.0 && env[i] - floor > 20.0 {
                    slopes.push((env[i] - env[i + 7]) / (7.0 * HOP_S));
                }
            }
        }
        if slopes.len() >= 5 {
            slopes.sort_by(f64::total_cmp);
            let s = slopes[slopes.len() / 2];
            // 200+ dB/s falls are dry; 80 dB/s and slower ring.
            h.room = Some(((1.0 - (s - 80.0) / 140.0).clamp(0.0, 1.0) * 100.0).round() / 100.0);
        }
    }
    // Rumble (below 70 Hz) where nothing musical is playing, and air.
    let quiet_kind = |w: usize| matches!(labels.get(w).copied(), Some("speech" | "noise" | "sound"));
    let on: Vec<&Feat> = fs.iter().enumerate().filter(|(i, f)| f.rms_db > -45.0 && quiet_kind(i / per_win)).map(|(_, f)| f).collect();
    if !on.is_empty() {
        let mean = |g: &dyn Fn(&Feat) -> f64| db(on.iter().map(|f| 10f64.powf(g(f) / 10.0)).sum::<f64>() / on.len() as f64);
        let mid = db((10f64.powf(mean(&|f| f.bands[2]) / 10.0) + 10f64.powf(mean(&|f| f.bands[3]) / 10.0)) / 2.0);
        h.rumble_db = ((mean(&|f| f.sub_db) - mid) * 10.0).round() / 10.0;
        h.air_db = ((mean(&|f| f.bands[5]) - mid) * 10.0).round() / 10.0;
    } else {
        h.rumble_db = -60.0;
    }
    h
}

/// What half a second sounds like, in numbers.
#[derive(Clone, Copy, Debug, Default)]
struct WinFeat {
    /// Frames with a pitch (of those sounding).
    voiced: f64,
    flatness: f64,
    flux: f64,
    /// Spread of the flux (onsets come and go).
    flux_sd: f64,
    /// Spectral peaks held from frame to frame.
    persistence: f64,
    /// Frames far quieter than the half second's mean (gaps between words).
    low_energy: f64,
    /// 3–8 Hz share of the speech-band envelope's movement over 2 s.
    m4: f64,
    /// Pitch movement between voiced frames (semitones, median).
    glide: f64,
    /// How far the energy below 150 Hz outweighs the rest (dB): a hum or a
    /// rumble alone.
    lowness: f64,
    /// The pitch only climbs (or only falls): a sweep, not a voice.
    sweep: bool,
}

fn window_feats(win: &[Feat], env: &[f64], around: &[Feat], loud: f64) -> WinFeat {
    let sounding: Vec<&Feat> = win.iter().filter(|f| f.rms_db > loud - 30.0).collect();
    let n = sounding.len().max(1) as f64;
    let voiced = sounding.iter().filter(|f| f.f0.is_some()).count() as f64 / n;
    let flatness = sounding.iter().map(|f| f.flatness).sum::<f64>() / n;
    let flux = sounding.iter().map(|f| f.flux).sum::<f64>() / n;
    let flux_sd = (sounding.iter().map(|f| (f.flux - flux).powi(2)).sum::<f64>() / n).sqrt();
    let mut kept = 0usize;
    let mut total = 0usize;
    for p in win.windows(2) {
        for &k in &p[0].peaks {
            total += 1;
            if p[1].peaks.iter().any(|&q| q.abs_diff(k) <= 1) {
                kept += 1;
            }
        }
    }
    // Gaps between words, in the band words live in (150 Hz - 4 kHz), so a
    // bright bed (crickets, hiss) or a rumble does not fill them.
    let lin: Vec<f64> = win.iter().map(|f| (1..=4).map(|b| 10f64.powf(f.bands[b] / 10.0)).sum::<f64>()).collect();
    let mean = lin.iter().sum::<f64>() / lin.len().max(1) as f64;
    let low_energy = lin.iter().filter(|&&v| v < mean * 0.25).count() as f64 / lin.len().max(1) as f64;
    let mut jumps: Vec<f64> = win.windows(2).filter_map(|p| Some((12.0 * (p[1].f0? / p[0].f0?).log2()).abs())).collect();
    jumps.sort_by(f64::total_cmp);
    let glide = jumps.get(jumps.len() / 2).copied().unwrap_or(0.0);
    // A sweep: over the two seconds around, the pitch only climbs (or only
    // falls) and covers a third or more (a riser, a siren's rise). A voice's
    // pitch goes up and down from phrase to phrase.
    let signed: Vec<f64> = around.windows(2).filter_map(|p| Some(12.0 * (p[1].f0? / p[0].f0?).log2())).filter(|j| j.abs() < 3.0).collect();
    let (up, down) = (signed.iter().filter(|j| **j > 0.02).map(|j| j.abs()).sum::<f64>(), signed.iter().filter(|j| **j < -0.02).map(|j| j.abs()).sum::<f64>());
    let sweep = signed.len() >= 20 && up.max(down) >= 4.0 && up.min(down) <= 0.15 * up.max(down);
    let band = |k: usize| db(sounding.iter().map(|f| 10f64.powf(f.bands[k] / 10.0)).sum::<f64>() / n);
    let lowness = band(0) - (1..6).map(band).fold(f64::NEG_INFINITY, f64::max);
    WinFeat { voiced, flatness, flux, flux_sd, persistence: kept as f64 / total.max(1) as f64, low_energy, m4: modulation(env, 3.0, 8.0), glide, lowness, sweep }
}

/// A label for half a second. Speech: voiced with gaps, pitch that moves,
/// spectra that change syllable by syllable; music: held notes and a
/// steady fill; noise and applause: flat spectra without pitch.
fn classify(f: &WinFeat) -> &'static str {
    if f.lowness > 15.0 {
        return "noise";
    }
    if f.flatness > 0.3 && f.voiced < 0.2 {
        return if f.flux_sd > 0.12 || f.low_energy > 0.25 { "applause" } else { "noise" };
    }
    // A voice: pitched, and the pitch moves (held synth notes do not).
    let talking = f.voiced > 0.3 && f.glide >= 0.08 && f.glide < 3.0 && !f.sweep;
    // Gaps between words: the level falls between syllables. Over music
    // the floor stays up.
    let gaps = f.low_energy > 0.15;
    if talking && gaps {
        return "speech";
    }
    if talking {
        // Over music a voice still has syllables: the 3-8 Hz movement.
        return if f.m4 > 0.3 { "speech over music" } else { "music" };
    }
    if f.voiced < 0.1 && f.flux < 0.08 && f.low_energy < 0.05 {
        // Pitchless but steady: a drone, a pad, room tone.
        return if f.flatness > 0.15 { "noise" } else { "music" };
    }
    if f.low_energy < 0.2 || f.persistence > 0.8 || (f.voiced > 0.3 && f.glide < 0.08) {
        return "music";
    }
    "sound"
}

/// Share of an envelope's variation between `lo` and `hi` Hz (0..1).
fn modulation(env: &[f64], lo: f64, hi: f64) -> f64 {
    let n = env.len();
    if n < 16 {
        return 0.0;
    }
    let mean = env.iter().sum::<f64>() / n as f64;
    let x: Vec<f64> = env.iter().map(|v| v - mean).collect();
    let fs = 1.0 / HOP_S;
    let (mut band, mut total) = (0.0, 0.0);
    for k in 1..n / 2 {
        let f = k as f64 * fs / n as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let a = -2.0 * std::f64::consts::PI * k as f64 * i as f64 / n as f64;
            re += v * a.cos();
            im += v * a.sin();
        }
        let p = re * re + im * im;
        if f <= 20.0 {
            total += p;
            if f >= lo && f <= hi {
                band += p;
            }
        }
    }
    if total <= 1e-9 { 0.0 } else { band / total }
}

/// Clicks and pops: one- or two-sample spikes standing far out of the
/// waveform around them (a drum hit is a fast but smooth rise, not a spike).
fn clicks(x: &[f32], rate: u32) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    if x.len() < 64 {
        return out;
    }
    // How far each sample is from the line through its neighbours.
    let e: Vec<f32> = (1..x.len() - 1).map(|i| (x[i] - 0.5 * (x[i - 1] + x[i + 1])).abs()).collect();
    let block = (rate as usize / 100).max(32); // 10 ms
    for (bi, b) in e.chunks(block).enumerate() {
        let mut s = b.to_vec();
        s.sort_by(f32::total_cmp);
        let med = s[s.len() / 2].max(1e-4);
        let (k, top) = b.iter().cloned().enumerate().fold((0, 0f32), |a, (i, v)| if v > a.1 { (i, v) } else { a });
        if top < 0.08 || top < med * 40.0 {
            continue;
        }
        // Narrow: three samples away it is back to the usual roughness.
        let at = bi * block + k;
        let side = |j: isize| e.get((at as isize + j).max(0) as usize).copied().unwrap_or(0.0);
        if side(-3).max(side(3)).max(side(-4)).max(side(4)) > top * 0.25 {
            continue;
        }
        let t = (at + 1) as f64 / rate as f64;
        if out.last().is_none_or(|&l| t - l > 0.2) {
            out.push((t * 100.0).round() / 100.0);
        }
    }
    out
}

/// Voice against music, while the voice speaks.
#[derive(Clone, Debug, Default)]
pub struct Balance {
    /// Median of (voice − music) in dB over the moments the voice speaks.
    pub voice_over_music_db: Option<f64>,
    /// Moments the music masks the voice: (from, to, the band).
    pub masked: Vec<(f64, f64, &'static str)>,
    /// Share of the speaking time that is masked.
    pub masked_share: f64,
}

/// How a voice track and a music track sit together (same rate, aligned).
pub fn balance(voice: &[f32], music: &[f32], rate: u32) -> Balance {
    let v = frames(&to_16k(voice, rate));
    let m = frames(&to_16k(music, rate));
    let n = v.len().min(m.len());
    let mut diffs = Vec::new();
    let mut masked_frames = vec![false; n];
    let mut band_of = vec![0usize; n];
    let mut speaking = 0usize;
    for i in 0..n {
        if v[i].rms_db < -42.0 || v[i].f0.is_none() && v[i].rms_db < -35.0 {
            continue;
        }
        speaking += 1;
        diffs.push(v[i].rms_db - m[i].rms_db);
        // 1–2 kHz and 2–4 kHz carry intelligibility.
        for k in [3usize, 4] {
            if m[i].bands[k] > v[i].bands[k] - 3.0 && m[i].bands[k] > -60.0 {
                masked_frames[i] = true;
                band_of[i] = k;
            }
        }
    }
    let mut b = Balance::default();
    if !diffs.is_empty() {
        diffs.sort_by(f64::total_cmp);
        b.voice_over_music_db = Some((diffs[diffs.len() / 2] * 10.0).round() / 10.0);
    }
    b.masked_share = masked_frames.iter().filter(|&&x| x).count() as f64 / speaking.max(1) as f64;
    // Stretches of at least 0.3 s.
    let mut i = 0;
    while i < n {
        if masked_frames[i] {
            let s = i;
            while i < n && masked_frames[i] {
                i += 1;
            }
            if (i - s) as f64 * HOP_S >= 0.3 {
                let band = if band_of[s] == 3 { "1–2 kHz" } else { "2–4 kHz" };
                let r2 = |t: f64| (t * 100.0).round() / 100.0;
                b.masked.push((r2(s as f64 * HOP_S), r2(i as f64 * HOP_S), band));
            }
        } else {
            i += 1;
        }
    }
    b
}

/// A few plain sentences about what was heard.
pub fn describe(h: &Heard) -> String {
    let mut s: Vec<String> = Vec::new();
    let total = h.duration.max(1e-6);
    let parts: Vec<String> = h.shares.iter().filter(|(_, d)| *d / total > 0.08).map(|(l, d)| format!("{l} {:.0}%", d / total * 100.0)).collect();
    if !parts.is_empty() {
        s.push(format!("Mostly {}.", parts.join(", ")));
    }
    if let Some((f0, spread)) = h.voice_pitch {
        let register = if f0 < 140.0 { "a low voice" } else if f0 < 200.0 { "a mid-pitched voice" } else { "a high voice" };
        let life = if spread < 1.5 { ", flat and even" } else if spread > 4.0 { ", lively, with a wide pitch range" } else { "" };
        let mut v = format!("{register} (around {f0:.0} Hz){life}");
        if let Some(r) = h.syllable_rate {
            v += &format!(", {} ({r:.1} syllables a second)", if r > 5.5 { "fast" } else if r < 3.0 { "slow and deliberate" } else { "at a natural pace" });
        }
        if let Some(room) = h.room {
            v += &format!(", {}", if room < 0.35 { "dry and close to the mic" } else if room < 0.65 { "with a little room sound" } else { "roomy and echoey (far from the mic, or a hard room)" });
        }
        s.push(format!("Speech: {v}."));
    }
    let mut faults = Vec::new();
    if h.clipped_runs > 0 {
        faults.push(format!("clipping ({} time(s), first at {:.2}s): distorted peaks that cannot be undone; lower the source or replace the take", h.clipped_runs, h.clipping.first().copied().unwrap_or(0.0)));
    }
    if !h.clicks.is_empty() {
        let at: Vec<String> = h.clicks.iter().take(5).map(|t| format!("{t:.2}s")).collect();
        faults.push(format!("{} click(s) or pop(s) at {}{}", h.clicks.len(), at.join(", "), if h.clicks.len() > 5 { "…" } else { "" }));
    }
    if let Some((hz, p)) = h.hum {
        faults.push(format!("a {hz:.0} Hz mains hum ({p:.0} dB above its neighbours): a notch or voice cleanup removes it"));
    }
    if h.rumble_db > 0.0 {
        faults.push(format!("low rumble (wind, traffic or handling; {:.0} dB against the mids): a high-pass at 80 Hz", h.rumble_db));
    }
    if faults.is_empty() {
        s.push("No clipping, clicks, hum or rumble heard.".into());
    } else {
        s.push(format!("Problems: {}.", faults.join("; ")));
    }
    if h.air_db < -30.0 && h.voice_pitch.is_some() {
        s.push("It sounds muffled (little above 4 kHz).".into());
    }
    s.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: u32 = 16_000;

    fn noise(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed % 20_001) as f32 / 10_000.0 - 1.0
    }

    /// Speech-like: a glottal pulse train whose pitch glides, through two
    /// moving formants, in 4.5 syllables a second with pauses.
    fn speechy(secs: f64) -> Vec<f32> {
        let n = (secs * R as f64) as usize;
        let mut out = vec![0f32; n];
        let mut phase = 0.0f64;
        let (mut y1, mut y2, mut z1, mut z2) = (0f64, 0f64, 0f64, 0f64);
        let mut seed = 7u64;
        for i in 0..n {
            let t = i as f64 / R as f64;
            let f0 = 140.0 + 30.0 * (t * 1.3).sin() + 15.0 * (t * 4.7).sin();
            phase += f0 / R as f64;
            let pulse = if phase >= 1.0 {
                phase -= 1.0;
                1.0
            } else {
                0.0
            };
            let syl = (t * 4.5 * std::f64::consts::PI).sin().abs().powf(1.5);
            let pause = if (t % 2.2) > 1.8 { 0.0 } else { 1.0 };
            let excite = pulse * syl * pause + 0.002 * noise(&mut seed) as f64;
            // Two resonators (formants) that move with the syllables.
            let f1 = 500.0 + 300.0 * (t * 9.0).sin();
            let f2 = 1500.0 + 600.0 * (t * 7.0).cos();
            let res = |f: f64, x: f64, a: &mut f64, b: &mut f64| {
                let r = 0.97;
                let c = 2.0 * r * (2.0 * std::f64::consts::PI * f / R as f64).cos();
                let y = x + c * *a - r * r * *b;
                *b = *a;
                *a = y;
                y
            };
            let s = res(f1, excite, &mut y1, &mut y2) + 0.6 * res(f2, excite, &mut z1, &mut z2);
            out[i] = (s * 0.05) as f32;
        }
        out
    }

    /// Music-like: held chords changing every beat with a soft kick.
    fn musicy(secs: f64) -> Vec<f32> {
        let n = (secs * R as f64) as usize;
        let chords = [[220.0, 277.2, 329.6], [196.0, 246.9, 293.7], [174.6, 220.0, 261.6], [196.0, 246.9, 311.1]];
        (0..n)
            .map(|i| {
                let t = i as f64 / R as f64;
                let c = chords[((t / 2.0) as usize) % 4];
                let tone: f64 = c.iter().map(|f| (2.0 * std::f64::consts::PI * f * t).sin() + 0.4 * (4.0 * std::f64::consts::PI * f * t).sin()).sum();
                let beat = t % 0.5;
                let kick = (-beat * 30.0).exp() * (2.0 * std::f64::consts::PI * 60.0 * beat).sin();
                (0.08 * tone + 0.3 * kick) as f32
            })
            .collect()
    }

    fn share(h: &Heard, label: &str) -> f64 {
        h.shares.iter().find(|s| s.0 == label).map_or(0.0, |s| s.1) / h.duration
    }

    #[test]
    fn dump() {
        if std::env::var_os("HEAR_DEBUG").is_none() {
            return;
        }
        for (name, x) in [("speech", speechy(4.0)), ("music", musicy(4.0))] {
            eprintln!("== {name} peak {}", x.iter().fold(0f32, |m, v| m.max(v.abs())));
            analyse(&x, R);
        }
        let mut seed = 3u64;
        eprintln!("== hiss");
        analyse(&(0..R * 3).map(|_| 0.1 * noise(&mut seed)).collect::<Vec<_>>(), R);
        for name in std::env::var("HEAR_FILES").unwrap_or_default().split(',').filter(|s| !s.is_empty()) {
            let Ok(w) = std::fs::read(name) else { continue };
            let x: Vec<f32> = w.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
            eprintln!("== {name}");
            let h = analyse(&x, R);
            eprintln!("{:?} {}", h.shares, describe(&h));
        }
    }

    #[test]
    fn speech_and_music_are_told_apart() {
        let s = analyse(&speechy(8.0), R);
        assert!(share(&s, "speech") > 0.6, "{:?}", s.shares);
        let m = analyse(&musicy(8.0), R);
        assert!(share(&m, "music") > 0.6, "{:?}", m.shares);
        assert!(s.voice_pitch.is_some_and(|(f, _)| (110.0..190.0).contains(&f)), "{:?}", s.voice_pitch);
    }

    #[test]
    fn silence_hum_clipping_and_clicks_are_found() {
        let mut x: Vec<f32> = (0..R * 6).map(|i| {
            let t = i as f64 / R as f64;
            (0.05 * (2.0 * std::f64::consts::PI * 60.0 * t).sin() + 0.03 * (2.0 * std::f64::consts::PI * 120.0 * t).sin()) as f32
        }).collect();
        // Two seconds of silence, a clipped burst, two clicks.
        for v in &mut x[R as usize * 4..] {
            *v = 0.0;
        }
        for (k, v) in x[R as usize..R as usize + 800].iter_mut().enumerate() {
            *v = ((k as f32 * 0.2).sin() * 3.0).clamp(-1.0, 1.0);
        }
        x[R as usize * 2 + 100] = 0.9;
        x[R as usize * 3 + 400] = -0.8;
        let h = analyse(&x, R);
        assert_eq!(h.hum.map(|h| h.0), Some(60.0), "{:?}", h.hum);
        assert!(h.clipped_runs > 0 && (h.clipping[0] - 1.0).abs() < 0.1, "{:?}", h.clipping);
        assert!(h.clicks.iter().any(|&t| (t - 2.0).abs() < 0.05) && h.clicks.iter().any(|&t| (t - 3.02).abs() < 0.05), "{:?}", h.clicks);
        assert!(h.segments.last().is_some_and(|s| s.label == "silence" && s.from <= 4.0), "{:?}", h.segments);
        let d = describe(&h);
        assert!(d.contains("60 Hz") && d.contains("clipping") && d.contains("click"), "{d}");
    }

    /// A bass line around 58 Hz (B flat, the note just under a mains
    /// hum's 60) with its harmonics, a beat of eighth notes and chords on
    /// top: music, not a fault.
    fn bass_line(secs: f64) -> Vec<f32> {
        let n = (secs * R as f64) as usize;
        let notes = [58.27, 58.27, 61.74, 55.0];
        let mut ph = 0.0f64;
        (0..n)
            .map(|i| {
                let t = i as f64 / R as f64;
                let bar = (t / 2.0) as usize;
                let hz = notes[bar % notes.len()];
                ph += hz / R as f64;
                let saw: f64 = (1..8).map(|k| (2.0 * std::f64::consts::PI * k as f64 * ph).sin() / k as f64).sum();
                let eighth = t % 0.25;
                let env = (-eighth / 0.18).exp();
                let chord: f64 = [233.1, 293.7, 349.2].iter().map(|f| (2.0 * std::f64::consts::PI * f * t).sin()).sum();
                (0.35 * saw * env + 0.05 * chord) as f32
            })
            .collect()
    }

    /// A bass note at about 58 Hz is a note, not a mains hum.
    #[test]
    fn a_bass_note_near_60_hz_is_not_hum() {
        let h = analyse(&bass_line(12.0), R);
        assert!(h.hum.is_none(), "{:?}", h.hum);
        // A held 58 Hz note, no rhythm at all, is not hum either: it is off the mains frequency.
        let held: Vec<f32> = (0..R * 8).map(|i| {
            let t = i as f64 / R as f64;
            (0.3 * (2.0 * std::f64::consts::PI * 58.3 * t).sin() + 0.1 * (2.0 * std::f64::consts::PI * 116.6 * t).sin()) as f32
        }).collect();
        assert!(analyse(&held, R).hum.is_none(), "{:?}", analyse(&held, R).hum);
    }

    /// Mains hum is heard under a voice and under music: 60 Hz with its
    /// harmonics, or a 50 Hz buzz that is mostly odd harmonics.
    #[test]
    fn mains_hum_under_voice_and_music_is_found() {
        let hum60 = |t: f64| 0.004 * (2.0 * std::f64::consts::PI * 60.0 * t).sin() + 0.003 * (2.0 * std::f64::consts::PI * 120.0 * t).sin() + 0.002 * (2.0 * std::f64::consts::PI * 180.0 * t).sin();
        let voice: Vec<f32> = speechy(8.0).iter().enumerate().map(|(i, v)| v + hum60(i as f64 / R as f64) as f32).collect();
        assert_eq!(analyse(&voice, R).hum.map(|h| h.0), Some(60.0), "{:?}", analyse(&voice, R).hum);
        let buzz50 = |t: f64| (1..8).step_by(2).map(|k| 0.01 / k as f64 * (2.0 * std::f64::consts::PI * 50.0 * k as f64 * t).sin()).sum::<f64>();
        let music: Vec<f32> = bass_line(10.0).iter().enumerate().map(|(i, v)| v + buzz50(i as f64 / R as f64) as f32).collect();
        assert_eq!(analyse(&music, R).hum.map(|h| h.0), Some(50.0), "{:?}", analyse(&music, R).hum);
    }

    /// Crickets: 4-5 kHz tone pulses, three to a chirp, a chirp every half
    /// second or so, three insects out of step (as the ambience makes them).
    fn crickets(secs: f64) -> Vec<f32> {
        let n = (secs * R as f64) as usize;
        let mut x = vec![0f32; n];
        for (c, (hz, every)) in [(4400.0, 0.47), (4750.0, 0.61), (5000.0, 0.83)].iter().enumerate() {
            let mut t = 0.05 * c as f64;
            while t < secs {
                for k in 0..3 {
                    let s = ((t + k as f64 * 0.03) * R as f64) as usize;
                    for i in 0..(0.018 * R as f64) as usize {
                        if s + i < n {
                            let u = i as f64 / (0.018 * R as f64);
                            x[s + i] += (0.2 * (std::f64::consts::PI * u).sin().powf(1.5) * (2.0 * std::f64::consts::PI * hz * i as f64 / R as f64).sin()) as f32;
                        }
                    }
                }
                t += every;
            }
        }
        x
    }

    /// A synth riser: noise swept up through a band-pass with a tone
    /// climbing three octaves under it, growing to the drop.
    fn riser(secs: f64) -> Vec<f32> {
        let n = (secs * R as f64) as usize;
        let mut seed = 5u64;
        let (mut lo, mut band) = (0f64, 0f64);
        let mut ph = 0f64;
        (0..n)
            .map(|i| {
                let p = i as f64 / n as f64;
                let hz = 250.0 * 24f64.powf(p);
                let fc = (2.0 * (std::f64::consts::PI * hz.min(R as f64 * 0.2) / R as f64).sin()).min(1.0);
                let hi = noise(&mut seed) as f64 - lo - 0.5 * band;
                band += fc * hi;
                lo += fc * band;
                ph += 110.0 * 8f64.powf(p) / R as f64;
                let tone: f64 = (1..6).map(|k| (2.0 * std::f64::consts::PI * k as f64 * ph).sin() / k as f64).sum();
                ((band + 0.3 * tone) * p * p * 0.3) as f32
            })
            .collect()
    }

    fn talk_share(h: &Heard) -> f64 {
        share(h, "speech") + share(h, "speech over music")
    }

    /// Crickets and a riser are not a voice: crickets have no pitch in
    /// the voice's range, and a riser's pitch only ever climbs.
    #[test]
    fn crickets_and_risers_are_not_speech() {
        let c = analyse(&crickets(8.0), R);
        assert!(talk_share(&c) < 0.1, "crickets: {:?}", c.shares);
        assert!(c.voice_pitch.is_none(), "crickets: {:?}", c.voice_pitch);
        for secs in [2.4, 6.0] {
            let r = analyse(&riser(secs), R);
            assert!(talk_share(&r) < 0.1, "riser {secs} s: {:?}", r.shares);
        }
        // And a voice over the crickets is still a voice.
        let v: Vec<f32> = speechy(8.0).iter().zip(crickets(8.0)).map(|(a, b)| a + 0.3 * b).collect();
        assert!(talk_share(&analyse(&v, R)) > 0.6, "{:?}", analyse(&v, R).shares);
    }

    #[test]
    fn noise_and_applause_are_not_speech() {
        let mut seed = 3u64;
        let hiss: Vec<f32> = (0..R * 4).map(|_| 0.1 * noise(&mut seed)).collect();
        let h = analyse(&hiss, R);
        assert!(share(&h, "noise") > 0.6, "{:?}", h.shares);
        // Applause: dense random claps (short noise bursts).
        let mut clap = vec![0f32; (R * 4) as usize];
        let mut s2 = 11u64;
        for _ in 0..120 {
            let at = (noise(&mut s2).abs() * (clap.len() - 400) as f32) as usize;
            for k in 0..300 {
                clap[at + k] += 0.4 * noise(&mut s2) * (-(k as f32) / 60.0).exp();
            }
        }
        let a = analyse(&clap, R);
        assert!(share(&a, "speech") < 0.2, "{:?}", a.shares);
    }

    #[test]
    fn balance_finds_music_too_loud_under_a_voice() {
        let v = speechy(6.0);
        // A bright bed: the chords with a buzzy lead up to 4 kHz.
        let bed: Vec<f32> = musicy(6.0).iter().enumerate().map(|(i, x)| {
            let t = i as f64 / R as f64;
            let lead: f64 = (1..14).map(|k| (2.0 * std::f64::consts::PI * 293.7 * k as f64 * t).sin() / k as f64).sum();
            x + (0.08 * lead) as f32
        }).collect();
        let quiet: Vec<f32> = bed.iter().map(|x| x * 0.05).collect();
        let loud: Vec<f32> = bed.iter().map(|x| x * 1.5).collect();
        let good = balance(&v, &quiet, R);
        let bad = balance(&v, &loud, R);
        assert!(good.voice_over_music_db.unwrap() > bad.voice_over_music_db.unwrap() + 15.0, "{:?} {:?}", good.voice_over_music_db, bad.voice_over_music_db);
        assert!(bad.masked_share > good.masked_share + 0.3, "{} {}", bad.masked_share, good.masked_share);
    }
}
