// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Audio DSP for model pre/post-processing: a mixed-radix complex FFT (any
//! length; Bluestein for large prime factors), real FFT helpers, windows,
//! STFT / inverse STFT with overlap-add, Slaney mel filter banks, Whisper's
//! log-mel spectrogram and a windowed-sinc resampler.

use std::f64::consts::PI;

// ------------------------------------------------------------------ FFT

/// A planned complex FFT of length `n` (unnormalised in both directions).
#[derive(Clone, Debug)]
pub struct Fft {
    n: usize,
    factors: Vec<usize>,
    /// exp(-2πik/n), k < n.
    tw_re: Vec<f32>,
    tw_im: Vec<f32>,
    bluestein: Option<Box<Bluestein>>,
}

#[derive(Clone, Debug)]
struct Bluestein {
    m: usize,
    inner: Fft,
    /// chirp exp(-iπk²/n), k < n.
    w_re: Vec<f32>,
    w_im: Vec<f32>,
    /// FFT of the conjugate chirp, zero padded to m.
    b_re: Vec<f32>,
    b_im: Vec<f32>,
}

fn factorize(mut n: usize) -> Vec<usize> {
    let mut f = Vec::new();
    for p in [4usize, 2, 3, 5] {
        while n % p == 0 && n > 1 {
            f.push(p);
            n /= p;
        }
    }
    let mut p = 7;
    while n > 1 {
        while n % p == 0 {
            f.push(p);
            n /= p;
        }
        p += 2;
        if p * p > n && n > 1 {
            f.push(n);
            break;
        }
    }
    f
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        let n = n.max(1);
        let factors = factorize(n);
        let largest = factors.iter().copied().max().unwrap_or(1);
        if largest > 61 {
            // Bluestein: convolution of length m >= 2n-1 with power-of-two FFTs.
            let m = (2 * n - 1).next_power_of_two();
            let inner = Fft::new(m);
            let mut w_re = vec![0.0; n];
            let mut w_im = vec![0.0; n];
            for k in 0..n {
                let kk = (k as u128 * k as u128 % (2 * n as u128)) as f64;
                let a = -PI * kk / n as f64;
                w_re[k] = a.cos() as f32;
                w_im[k] = a.sin() as f32;
            }
            let mut b_re = vec![0.0; m];
            let mut b_im = vec![0.0; m];
            b_re[0] = w_re[0];
            b_im[0] = -w_im[0];
            for k in 1..n {
                b_re[k] = w_re[k];
                b_im[k] = -w_im[k];
                b_re[m - k] = w_re[k];
                b_im[m - k] = -w_im[k];
            }
            inner.run(&mut b_re, &mut b_im, false);
            return Fft { n, factors, tw_re: Vec::new(), tw_im: Vec::new(), bluestein: Some(Box::new(Bluestein { m, inner, w_re, w_im, b_re, b_im })) };
        }
        let mut tw_re = vec![0.0; n];
        let mut tw_im = vec![0.0; n];
        for k in 0..n {
            let a = -2.0 * PI * k as f64 / n as f64;
            tw_re[k] = a.cos() as f32;
            tw_im[k] = a.sin() as f32;
        }
        Fft { n, factors, tw_re, tw_im, bluestein: None }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// In-place transform. Forward uses exp(-i…), inverse exp(+i…); neither
    /// is scaled.
    pub fn run(&self, re: &mut [f32], im: &mut [f32], inverse: bool) {
        let n = self.n;
        assert!(re.len() >= n && im.len() >= n, "Fft::run: buffers shorter than {n}");
        if n == 1 {
            return;
        }
        if inverse {
            // ifft(x) = conj(fft(conj(x)))
            for v in im[..n].iter_mut() {
                *v = -*v;
            }
            self.run(re, im, false);
            for v in im[..n].iter_mut() {
                *v = -*v;
            }
            return;
        }
        if let Some(b) = &self.bluestein {
            let m = b.m;
            let mut ar = vec![0.0f32; m];
            let mut ai = vec![0.0f32; m];
            for k in 0..n {
                ar[k] = re[k] * b.w_re[k] - im[k] * b.w_im[k];
                ai[k] = re[k] * b.w_im[k] + im[k] * b.w_re[k];
            }
            b.inner.run(&mut ar, &mut ai, false);
            for k in 0..m {
                let (r, i) = (ar[k] * b.b_re[k] - ai[k] * b.b_im[k], ar[k] * b.b_im[k] + ai[k] * b.b_re[k]);
                ar[k] = r;
                ai[k] = i;
            }
            b.inner.run(&mut ar, &mut ai, true);
            let s = 1.0 / m as f32;
            for k in 0..n {
                let (r, i) = (ar[k] * s, ai[k] * s);
                re[k] = r * b.w_re[k] - i * b.w_im[k];
                im[k] = r * b.w_im[k] + i * b.w_re[k];
            }
            return;
        }
        let src_re = re[..n].to_vec();
        let src_im = im[..n].to_vec();
        let mut scratch = vec![(0.0f32, 0.0f32); self.factors.iter().copied().max().unwrap_or(1)];
        self.work(&mut re[..n], &mut im[..n], &src_re, &src_im, 0, 1, 0, &mut scratch);
    }

    /// kissfft-style decimation in time: `out` (length m·p at this level)
    /// receives the DFT of input samples `src[off + j·stride]`.
    #[allow(clippy::too_many_arguments)]
    fn work(&self, out_re: &mut [f32], out_im: &mut [f32], src_re: &[f32], src_im: &[f32], off: usize, stride: usize, fi: usize, scratch: &mut [(f32, f32)]) {
        let p = self.factors[fi];
        let len = out_re.len();
        let m = len / p;
        if m == 1 {
            for j in 0..p {
                out_re[j] = src_re[off + j * stride];
                out_im[j] = src_im[off + j * stride];
            }
        } else {
            for j in 0..p {
                let (r, i) = (&mut out_re[j * m..(j + 1) * m], &mut out_im[j * m..(j + 1) * m]);
                self.work(r, i, src_re, src_im, off + j * stride, stride * p, fi + 1, scratch);
            }
        }
        // twiddle step for this level: index multiplier = stride (= n / len)
        let n = self.n;
        let fstride = n / len;
        match p {
            2 => {
                for u in 0..m {
                    let t = u * fstride;
                    let (wr, wi) = (self.tw_re[t], self.tw_im[t]);
                    let (br, bi) = (out_re[u + m], out_im[u + m]);
                    let (tr, ti) = (br * wr - bi * wi, br * wi + bi * wr);
                    let (ar, ai) = (out_re[u], out_im[u]);
                    out_re[u] = ar + tr;
                    out_im[u] = ai + ti;
                    out_re[u + m] = ar - tr;
                    out_im[u + m] = ai - ti;
                }
            }
            4 => {
                for u in 0..m {
                    let mut x = [(0.0f32, 0.0f32); 4];
                    for (q, xq) in x.iter_mut().enumerate() {
                        let t = (q * u * fstride) % n;
                        let (r, i) = (out_re[u + q * m], out_im[u + q * m]);
                        *xq = (r * self.tw_re[t] - i * self.tw_im[t], r * self.tw_im[t] + i * self.tw_re[t]);
                    }
                    let (a0, a1) = ((x[0].0 + x[2].0, x[0].1 + x[2].1), (x[0].0 - x[2].0, x[0].1 - x[2].1));
                    let (b0, b1) = ((x[1].0 + x[3].0, x[1].1 + x[3].1), (x[1].0 - x[3].0, x[1].1 - x[3].1));
                    // forward: multiply b1 by -i
                    let b1j = (b1.1, -b1.0);
                    out_re[u] = a0.0 + b0.0;
                    out_im[u] = a0.1 + b0.1;
                    out_re[u + m] = a1.0 + b1j.0;
                    out_im[u + m] = a1.1 + b1j.1;
                    out_re[u + 2 * m] = a0.0 - b0.0;
                    out_im[u + 2 * m] = a0.1 - b0.1;
                    out_re[u + 3 * m] = a1.0 - b1j.0;
                    out_im[u + 3 * m] = a1.1 - b1j.1;
                }
            }
            _ => {
                for u in 0..m {
                    for q in 0..p {
                        scratch[q] = (out_re[u + q * m], out_im[u + q * m]);
                    }
                    for q1 in 0..p {
                        let k = u + q1 * m;
                        let (mut sr, mut si) = (0.0f32, 0.0f32);
                        for (q, &(r, i)) in scratch[..p].iter().enumerate() {
                            let t = (q * k * fstride) % n;
                            sr += r * self.tw_re[t] - i * self.tw_im[t];
                            si += r * self.tw_im[t] + i * self.tw_re[t];
                        }
                        out_re[k] = sr;
                        out_im[k] = si;
                    }
                }
            }
        }
    }

    /// Forward FFT of a real signal; returns the `n/2+1` non-negative bins.
    pub fn rfft(&self, x: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let n = self.n;
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        re[..x.len().min(n)].copy_from_slice(&x[..x.len().min(n)]);
        self.run(&mut re, &mut im, false);
        re.truncate(n / 2 + 1);
        im.truncate(n / 2 + 1);
        (re, im)
    }

    /// Inverse of [`Fft::rfft`] (scaled by 1/n): `n/2+1` bins → `n` samples.
    pub fn irfft(&self, re: &[f32], im: &[f32]) -> Vec<f32> {
        let n = self.n;
        let h = n / 2 + 1;
        let mut fr = vec![0.0; n];
        let mut fi = vec![0.0; n];
        for k in 0..h.min(re.len()) {
            fr[k] = re[k];
            fi[k] = im[k];
        }
        for k in h..n {
            fr[k] = fr[n - k];
            fi[k] = -fi[n - k];
        }
        self.run(&mut fr, &mut fi, true);
        let s = 1.0 / n as f32;
        fr.iter().map(|v| v * s).collect()
    }
}

// --------------------------------------------------------------- windows

/// Periodic Hann window (torch.hann_window default).
pub fn hann_window(n: usize) -> Vec<f32> {
    (0..n).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32).collect()
}

/// Vorbis power-complementary window (DeepFilterNet).
pub fn vorbis_window(n: usize) -> Vec<f32> {
    let h = n as f64 / 2.0;
    (0..n)
        .map(|i| {
            let s = (0.5 * PI * (i as f64 + 0.5) / h).sin();
            (0.5 * PI * s * s).sin() as f32
        })
        .collect()
}

// ------------------------------------------------------------------ STFT

/// A complex spectrogram: `frames × bins`, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrogram {
    pub frames: usize,
    pub bins: usize,
    pub re: Vec<f32>,
    pub im: Vec<f32>,
}

impl Spectrogram {
    pub fn power(&self) -> Vec<f32> {
        self.re.iter().zip(&self.im).map(|(r, i)| r * r + i * i).collect()
    }
    pub fn magnitude(&self) -> Vec<f32> {
        self.re.iter().zip(&self.im).map(|(r, i)| (r * r + i * i).sqrt()).collect()
    }
}

fn reflect_index(i: isize, n: usize) -> usize {
    if n == 1 {
        return 0;
    }
    let period = 2 * (n as isize - 1);
    let mut j = i.rem_euclid(period);
    if j >= n as isize {
        j = period - j;
    }
    j as usize
}

/// Short-time Fourier transform. With `center`, the signal is reflect-padded
/// by `n_fft/2` on both sides (torch.stft default); frames are
/// `1 + (len_padded - n_fft) / hop`.
pub fn stft(x: &[f32], n_fft: usize, hop: usize, window: &[f32], center: bool) -> Spectrogram {
    let fft = Fft::new(n_fft);
    let pad = if center { n_fft / 2 } else { 0 };
    let total = x.len() + 2 * pad;
    let frames = if total >= n_fft { 1 + (total - n_fft) / hop } else { 0 };
    let bins = n_fft / 2 + 1;
    let mut out = Spectrogram { frames, bins, re: vec![0.0; frames * bins], im: vec![0.0; frames * bins] };
    let mut re = vec![0.0f32; n_fft];
    let mut im = vec![0.0f32; n_fft];
    let wl = window.len().min(n_fft);
    let woff = (n_fft - wl) / 2;
    for f in 0..frames {
        for k in 0..n_fft {
            let i = (f * hop + k) as isize - pad as isize;
            let v = if x.is_empty() { 0.0 } else if i < 0 || i >= x.len() as isize { x[reflect_index(i, x.len())] } else { x[i as usize] };
            let w = if k >= woff && k < woff + wl { window[k - woff] } else { 0.0 };
            re[k] = v * w;
            im[k] = 0.0;
        }
        fft.run(&mut re, &mut im, false);
        out.re[f * bins..(f + 1) * bins].copy_from_slice(&re[..bins]);
        out.im[f * bins..(f + 1) * bins].copy_from_slice(&im[..bins]);
    }
    out
}

/// Inverse STFT by weighted overlap-add (window-square normalised), the
/// inverse of [`stft`] with the same parameters. `length` trims the output.
pub fn istft(s: &Spectrogram, n_fft: usize, hop: usize, window: &[f32], center: bool, length: Option<usize>) -> Vec<f32> {
    let fft = Fft::new(n_fft);
    let total = n_fft + hop * s.frames.saturating_sub(1);
    let mut y = vec![0.0f32; total];
    let mut wsum = vec![0.0f32; total];
    let wl = window.len().min(n_fft);
    let woff = (n_fft - wl) / 2;
    let win = |k: usize| if k >= woff && k < woff + wl { window[k - woff] } else { 0.0 };
    for f in 0..s.frames {
        let frame = fft.irfft(&s.re[f * s.bins..(f + 1) * s.bins], &s.im[f * s.bins..(f + 1) * s.bins]);
        for k in 0..n_fft {
            let w = win(k);
            y[f * hop + k] += frame[k] * w;
            wsum[f * hop + k] += w * w;
        }
    }
    for (v, w) in y.iter_mut().zip(&wsum) {
        if *w > 1e-11 {
            *v /= w;
        }
    }
    let pad = if center { n_fft / 2 } else { 0 };
    let mut out: Vec<f32> = y.into_iter().skip(pad).collect();
    let want = length.unwrap_or(total.saturating_sub(2 * pad));
    out.resize(want, 0.0);
    out
}

// ------------------------------------------------------------------ mel

fn hz_to_mel_slaney(f: f64) -> f64 {
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if f >= min_log_hz { min_log_mel + (f / min_log_hz).ln() / logstep } else { f / f_sp }
}

fn mel_to_hz_slaney(m: f64) -> f64 {
    let (f_sp, min_log_hz) = (200.0 / 3.0, 1000.0);
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if m >= min_log_mel { min_log_hz * (logstep * (m - min_log_mel)).exp() } else { f_sp * m }
}

/// librosa `filters.mel(sr, n_fft, n_mels, fmin, fmax, htk=False,
/// norm='slaney')`: `n_mels × (n_fft/2+1)`, row-major.
pub fn mel_filters(sr: f32, n_fft: usize, n_mels: usize, fmin: f32, fmax: f32) -> Vec<f32> {
    let bins = n_fft / 2 + 1;
    let fft_freqs: Vec<f64> = (0..bins).map(|i| i as f64 * sr as f64 / n_fft as f64).collect();
    let (mmin, mmax) = (hz_to_mel_slaney(fmin as f64), hz_to_mel_slaney(fmax as f64));
    let mel_f: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz_slaney(mmin + (mmax - mmin) * i as f64 / (n_mels + 1) as f64)).collect();
    let mut w = vec![0.0f32; n_mels * bins];
    for m in 0..n_mels {
        let (lo, ce, hi) = (mel_f[m], mel_f[m + 1], mel_f[m + 2]);
        let enorm = 2.0 / (hi - lo);
        for (b, &f) in fft_freqs.iter().enumerate() {
            let lower = (f - lo) / (ce - lo);
            let upper = (hi - f) / (hi - ce);
            let v = lower.min(upper).max(0.0);
            w[m * bins + b] = (v * enorm) as f32;
        }
    }
    w
}

/// Whisper sample rate, FFT size, hop and window length.
pub const WHISPER_SR: usize = 16_000;
pub const WHISPER_N_FFT: usize = 400;
pub const WHISPER_HOP: usize = 160;
pub const WHISPER_CHUNK_SAMPLES: usize = 30 * WHISPER_SR;
pub const WHISPER_FRAMES: usize = WHISPER_CHUNK_SAMPLES / WHISPER_HOP;

/// Whisper's log-mel spectrogram of 16 kHz mono audio, padded/trimmed to
/// 30 s: `n_mels × 3000`, row-major. Matches `whisper.audio.log_mel_spectrogram`
/// (periodic Hann, centred reflect-padded STFT, last frame dropped,
/// log10 clamp to max−8, then (x+4)/4).
pub fn whisper_log_mel(samples: &[f32], n_mels: usize) -> Vec<f32> {
    let mut x = samples[..samples.len().min(WHISPER_CHUNK_SAMPLES)].to_vec();
    x.resize(WHISPER_CHUNK_SAMPLES, 0.0);
    log_mel(&x, n_mels, Some(WHISPER_FRAMES))
}

/// Log-mel of arbitrary-length 16 kHz audio (no 30 s padding); `frames`
/// forces the frame count.
pub fn log_mel(x: &[f32], n_mels: usize, frames: Option<usize>) -> Vec<f32> {
    let win = hann_window(WHISPER_N_FFT);
    let spec = stft(x, WHISPER_N_FFT, WHISPER_HOP, &win, true);
    let nf = frames.unwrap_or(spec.frames.saturating_sub(1)).min(spec.frames);
    let filt = mel_filters(WHISPER_SR as f32, WHISPER_N_FFT, n_mels, 0.0, WHISPER_SR as f32 / 2.0);
    let bins = spec.bins;
    let power = spec.power();
    let mut out = vec![0.0f32; n_mels * nf];
    for m in 0..n_mels {
        let fr = &filt[m * bins..(m + 1) * bins];
        for f in 0..nf {
            let p = &power[f * bins..(f + 1) * bins];
            let v: f32 = fr.iter().zip(p).map(|(a, b)| a * b).sum();
            out[m * nf + f] = v.max(1e-10).log10();
        }
    }
    let mx = out.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    for v in out.iter_mut() {
        *v = ((*v).max(mx - 8.0) + 4.0) / 4.0;
    }
    out
}

// -------------------------------------------------------------- resample

/// Band-limited resampling with a Hann-windowed sinc (16 zero crossings).
pub fn resample(x: &[f32], from_hz: u32, to_hz: u32) -> Vec<f32> {
    if from_hz == to_hz || x.is_empty() {
        return x.to_vec();
    }
    let ratio = to_hz as f64 / from_hz as f64;
    let out_len = ((x.len() as f64) * ratio).round() as usize;
    let cutoff = ratio.min(1.0);
    let half = 16.0 / cutoff;
    let mut out = vec![0.0f32; out_len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f64 / ratio;
        let lo = (t - half).ceil().max(0.0) as usize;
        let hi = ((t + half).floor() as usize).min(x.len() - 1);
        let (mut acc, mut wsum) = (0.0f64, 0.0f64);
        for (j, &xj) in x.iter().enumerate().take(hi + 1).skip(lo) {
            let d = j as f64 - t;
            let s = if d.abs() < 1e-9 { 1.0 } else { (PI * d * cutoff).sin() / (PI * d * cutoff) };
            let w = 0.5 + 0.5 * (PI * d / half).cos();
            let k = s * w * cutoff;
            acc += xj as f64 * k;
            wsum += k;
        }
        // normalise by the kernel sum so DC passes at unity gain
        *o = if wsum.abs() > 1e-12 { (acc / wsum) as f32 } else { 0.0 };
    }
    out
}

/// Interleaved multi-channel → mono average.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved.chunks_exact(channels).map(|c| c.iter().sum::<f32>() / channels as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_dft(re: &[f32], im: &[f32], inverse: bool) -> (Vec<f32>, Vec<f32>) {
        let n = re.len();
        let sg = if inverse { 1.0 } else { -1.0 };
        let mut or = vec![0.0; n];
        let mut oi = vec![0.0; n];
        for k in 0..n {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for j in 0..n {
                let a = sg * 2.0 * PI * (j * k) as f64 / n as f64;
                sr += re[j] as f64 * a.cos() - im[j] as f64 * a.sin();
                si += re[j] as f64 * a.sin() + im[j] as f64 * a.cos();
            }
            or[k] = sr as f32;
            oi[k] = si as f32;
        }
        (or, oi)
    }

    #[test]
    fn fft_matches_naive_dft_for_many_lengths() {
        for &n in &[1usize, 2, 3, 4, 5, 6, 7, 8, 12, 16, 30, 49, 64, 97, 100, 128, 210, 400, 480, 960, 1031] {
            let re: Vec<f32> = (0..n).map(|i| ((i * 7 + 3) % 11) as f32 / 11.0 - 0.5).collect();
            let im: Vec<f32> = (0..n).map(|i| ((i * 5 + 1) % 13) as f32 / 13.0 - 0.5).collect();
            for inverse in [false, true] {
                let (er, ei) = naive_dft(&re, &im, inverse);
                let (mut r, mut i) = (re.clone(), im.clone());
                Fft::new(n).run(&mut r, &mut i, inverse);
                let tol = 1e-3 * (n as f32).sqrt();
                for k in 0..n {
                    assert!((r[k] - er[k]).abs() < tol && (i[k] - ei[k]).abs() < tol, "n={n} k={k} inv={inverse}: {} {} vs {} {}", r[k], i[k], er[k], ei[k]);
                }
            }
        }
    }

    #[test]
    fn rfft_irfft_roundtrip() {
        for &n in &[16usize, 400, 960] {
            let x: Vec<f32> = (0..n).map(|i| (i as f32 * 0.37).sin()).collect();
            let f = Fft::new(n);
            let (r, i) = f.rfft(&x);
            let y = f.irfft(&r, &i);
            for k in 0..n {
                assert!((x[k] - y[k]).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn stft_istft_roundtrip() {
        let x: Vec<f32> = (0..4000).map(|i| (i as f32 * 0.05).sin() + 0.3 * (i as f32 * 0.31).cos()).collect();
        for (n_fft, hop, win) in [(400, 160, hann_window(400)), (960, 480, vorbis_window(960))] {
            let s = stft(&x, n_fft, hop, &win, true);
            let y = istft(&s, n_fft, hop, &win, true, Some(x.len()));
            for k in 0..x.len() {
                assert!((x[k] - y[k]).abs() < 2e-3, "n_fft {n_fft} k {k}: {} vs {}", x[k], y[k]);
            }
        }
    }

    #[test]
    fn stft_frame_count_matches_torch() {
        // torch.stft(center=True): 1 + len // hop
        let s = stft(&vec![0.0; 480_000], 400, 160, &hann_window(400), true);
        assert_eq!(s.frames, 3001);
        assert_eq!(s.bins, 201);
    }

    #[test]
    fn vorbis_window_is_power_complementary() {
        let w = vorbis_window(960);
        for i in 0..480 {
            assert!((w[i] * w[i] + w[i + 480] * w[i + 480] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn mel_filters_match_librosa_reference() {
        // librosa.filters.mel(sr=16000, n_fft=400, n_mels=80) known values
        let f = mel_filters(16000.0, 400, 80, 0.0, 8000.0);
        assert_eq!(f.len(), 80 * 201);
        // first filter peaks at bin 1 with height 2/(f2-f0) = 2/(53.33..)
        assert!((f[1] - 0.024_862_33).abs() < 2e-4, "{}", f[1]);
        assert_eq!(f[0], 0.0);
        // each filter is non-negative and non-empty
        for m in 0..80 {
            let row = &f[m * 201..(m + 1) * 201];
            assert!(row.iter().all(|&v| v >= 0.0));
            assert!(row.iter().any(|&v| v > 0.0), "filter {m} empty");
        }
    }

    #[test]
    fn whisper_log_mel_shape_and_range() {
        let x: Vec<f32> = (0..16000).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 16000.0).sin() * 0.5).collect();
        let m = whisper_log_mel(&x, 128);
        assert_eq!(m.len(), 128 * 3000);
        let mx = m.iter().copied().fold(f32::MIN, f32::max);
        let mn = m.iter().copied().fold(f32::MAX, f32::min);
        assert!(mx <= 1.0 + 4.0 / 4.0 + 10.0);
        assert!((mx - mn - 2.0).abs() < 1e-4, "dynamic range is clamped to 8 log10 units: {mx} {mn}");
        // the 440 Hz tone lands in the expected mel band (strongest early frame row)
        let col = 10;
        let best = (0..128).max_by(|&a, &b| m[a * 3000 + col].partial_cmp(&m[b * 3000 + col]).unwrap()).unwrap();
        let filt = mel_filters(16000.0, 400, 128, 0.0, 8000.0);
        let bin440 = (440.0f32 / 40.0).round() as usize;
        assert!(filt[best * 201 + bin440] > 0.0 || filt[best * 201 + bin440 + 1] > 0.0 || filt[best * 201 + bin440 - 1] > 0.0);
    }

    #[test]
    fn resample_preserves_tone() {
        let x: Vec<f32> = (0..48000).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin()).collect();
        let y = resample(&x, 48000, 16000);
        assert_eq!(y.len(), 16000);
        for i in 100..15900 {
            let e = (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 16000.0).sin();
            assert!((y[i] - e).abs() < 0.02, "{i}: {} vs {e}", y[i]);
        }
    }
}
