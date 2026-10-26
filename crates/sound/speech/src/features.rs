// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the aligner listens for: every 10 ms of a voice described by how
//! loud it is, how much of it is high hiss, how much is low hum and how
//! voiced (periodic) it is.

/// Frames per second of every feature track.
pub const FPS: f64 = 100.0;

#[derive(Clone, Debug, Default)]
pub struct Frames {
    /// Loudness, 0 (the floor) to 1 (the loud vowels).
    pub loud: Vec<f32>,
    /// Share of the energy above about 3.5 kHz (fricatives, bursts).
    pub hf: Vec<f32>,
    /// Share of the energy below 700 Hz (hum of nasals and closures).
    pub lf: Vec<f32>,
    /// Periodicity, 0 noise … 1 a clean pitch.
    pub voice: Vec<f32>,
    /// Raw level in dB.
    pub db: Vec<f32>,
}

impl Frames {
    pub fn len(&self) -> usize {
        self.loud.len()
    }
    pub fn is_empty(&self) -> bool {
        self.loud.is_empty()
    }
}

/// Averages `mono` down to about 16 kHz; returns the samples and rate.
fn to_16k(mono: &[f32], rate: u32) -> (Vec<f32>, u32) {
    let k = ((rate as f64 / 16_000.0).round() as usize).max(1);
    if k == 1 {
        return (mono.to_vec(), rate);
    }
    let out = mono.chunks(k).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    (out, rate / k as u32)
}

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for s in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (s + k, s + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

/// The feature frames of a voice.
pub fn analyse(mono: &[f32], rate: u32) -> Frames {
    let (x, r) = to_16k(mono, rate);
    let hop = (r as f64 / FPS).round() as usize;
    let win = 400usize.min(hop * 3);
    let nfft = 512;
    if x.len() < win || hop == 0 {
        return Frames::default();
    }
    let hann: Vec<f32> = (0..win).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / win as f32).cos()).collect();
    let bin = |hz: f32| ((hz / r as f32 * nfft as f32) as usize).min(nfft / 2);
    let (b_lo, b_hi) = (bin(700.0), bin(3500.0f32.min(0.4 * r as f32)));
    let (lag_lo, lag_hi) = ((r as f32 / 350.0) as usize, ((r as f32 / 70.0) as usize).min(win - 2));
    let mut f = Frames::default();
    let n = x.len().saturating_sub(win) / hop + 1;
    let (mut re, mut im) = (vec![0.0f32; nfft], vec![0.0f32; nfft]);
    let mut seg = vec![0.0f32; win];
    for k in 0..n {
        // Centre the window on the frame time.
        let c = k * hop + hop / 2;
        let s = c.saturating_sub(win / 2).min(x.len() - win);
        let raw = &x[s..s + win];
        let mean = raw.iter().sum::<f32>() / win as f32;
        for i in 0..win {
            seg[i] = (raw[i] - mean) * hann[i];
        }
        let e = seg.iter().map(|v| v * v).sum::<f32>() / win as f32;
        f.db.push(10.0 * (e + 1e-10).log10());
        re.iter_mut().for_each(|v| *v = 0.0);
        im.iter_mut().for_each(|v| *v = 0.0);
        re[..win].copy_from_slice(&seg);
        fft(&mut re, &mut im);
        let p: Vec<f32> = (0..=nfft / 2).map(|i| re[i] * re[i] + im[i] * im[i]).collect();
        let tot: f32 = p[1..].iter().sum::<f32>() + 1e-12;
        f.lf.push(p[1..=b_lo.max(1)].iter().sum::<f32>() / tot);
        f.hf.push(p[b_hi..].iter().sum::<f32>() / tot);
        let r0: f32 = seg.iter().map(|v| v * v).sum::<f32>() + 1e-12;
        let mut best = 0.0f32;
        for lag in lag_lo..=lag_hi {
            let mut a = 0.0;
            for i in 0..win - lag {
                a += seg[i] * seg[i + lag];
            }
            best = best.max(a / r0);
        }
        f.voice.push(best.clamp(0.0, 1.0));
    }
    let mut sorted = f.db.clone();
    sorted.sort_by(f32::total_cmp);
    let pc = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    let peak = pc(0.95);
    let floor = (pc(0.05) + 6.0).max(peak - 45.0).min(peak - 15.0);
    f.loud = f.db.iter().map(|&d| ((d - floor) / (peak - floor)).clamp(0.0, 1.0)).collect();
    f
}
