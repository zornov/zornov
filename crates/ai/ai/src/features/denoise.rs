// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Voice cleanup: removes background noise from speech with DeepFilterNet 3,
//! offline, into a new audio file the timeline then plays instead of the
//! original. Without the model, a classical spectral gate does a milder job.
//!
//! DeepFilterNet works on a 48 kHz STFT (20 ms window, 10 ms hop). Its
//! encoder reads two feature sets — log energies in 32 ERB bands and the
//! normalised complex spectrum of the lowest 96 bins (0–4.8 kHz) — and two
//! decoders produce (a) a gain per ERB band, applied to the whole spectrum,
//! and (b) "deep filter" coefficients: for each of the 96 low bins a 5-tap
//! complex filter over neighbouring frames, which restores the speech
//! harmonics there. Schröter et al., "DeepFilterNet: Perceptually Motivated
//! Real-Time Speech Enhancement" (2023). Weights: MIT / Apache-2.0.

use crate::engine::{self, Tensor};
use crate::error::{invalid, Result};
use crate::jobs::JobCtx;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::audio::{resample, vorbis_window, Fft};
use studio_nn::Session;

/// Model constants (from the model's `config.ini`).
const SR: u32 = 48_000;
const FFT: usize = 960;
const HOP: usize = 480;
const BINS: usize = FFT / 2 + 1;
const NB_ERB: usize = 32;
const NB_DF: usize = 96;
const DF_ORDER: usize = 5;
const DF_LOOKAHEAD: usize = 2;
/// The exported networks see `CONV_LOOKAHEAD` frames ahead: output frame
/// `f + CONV_LOOKAHEAD` belongs to spectrum frame `f`.
const CONV_LOOKAHEAD: usize = 2;
const MIN_ERB_FREQS: usize = 2;
/// Feature normalisation decay: exp(−hop / sr / τ) with τ = 1 s, rounded
/// the way the reference implementation rounds it.
const NORM_ALPHA: f32 = 0.99;

#[derive(Clone, Debug, PartialEq)]
pub struct CleanupSettings {
    /// 0 = untouched, 1 = full noise removal. In between, the noise is
    /// turned down rather than removed, which sounds more natural.
    pub strength: f32,
}

impl Default for CleanupSettings {
    fn default() -> Self {
        CleanupSettings { strength: 1.0 }
    }
}

impl CleanupSettings {
    pub fn cache_params(&self, model: bool) -> String {
        format!("v1 {} strength={:.3}", if model { "dfn3" } else { "gate" }, self.strength)
    }

    /// Strength → largest attenuation in dB (0 → 0 dB, 1 → unlimited).
    fn max_attenuation_db(&self) -> Option<f32> {
        let s = self.strength.clamp(0.0, 1.0);
        if s >= 0.999 { None } else { Some(s * 36.0) }
    }
}

/// Which engine processed the audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupEngine {
    DeepFilterNet,
    SpectralGate,
}

pub struct VoiceCleanup {
    model: Option<Dfn>,
    /// Network frames per inference chunk (30 s); see [`Dfn::enhance`].
    chunk: usize,
}

struct Dfn {
    enc: Arc<Session>,
    erb_dec: Arc<Session>,
    df_dec: Arc<Session>,
}

impl VoiceCleanup {
    /// Loads DeepFilterNet if installed; otherwise the spectral-gate
    /// fallback is used (never fails for a missing model).
    pub fn load(store: &ModelStore) -> Result<VoiceCleanup> {
        let model = match store.require(ModelId::DeepFilterNet3) {
            Ok(dir) => Some(Dfn { enc: engine::session(&dir.join("enc.onnx"))?, erb_dec: engine::session(&dir.join("erb_dec.onnx"))?, df_dec: engine::session(&dir.join("df_dec.onnx"))? }),
            Err(_) => None,
        };
        Ok(VoiceCleanup { model, chunk: 3000 })
    }

    /// The classical fallback only.
    pub fn fallback() -> VoiceCleanup {
        VoiceCleanup { model: None, chunk: 3000 }
    }

    /// Smaller inference chunks (for testing the chunk seams).
    #[doc(hidden)]
    pub fn set_chunk_frames(&mut self, frames: usize) {
        self.chunk = frames.max(50);
    }

    pub fn engine(&self) -> CleanupEngine {
        if self.model.is_some() { CleanupEngine::DeepFilterNet } else { CleanupEngine::SpectralGate }
    }

    /// Cleans interleaved audio (any rate, any channel count); returns audio
    /// in the same format and length.
    pub fn process(&self, interleaved: &[f32], rate: u32, channels: usize, settings: &CleanupSettings, ctx: &JobCtx) -> Result<Vec<f32>> {
        if channels == 0 || rate == 0 || interleaved.len() % channels != 0 {
            return invalid("bad audio layout");
        }
        let frames = interleaved.len() / channels;
        let mut out = vec![0.0f32; interleaved.len()];
        for c in 0..channels {
            let sub = ctx.range(c as f32 / channels as f32, (c + 1) as f32 / channels as f32);
            ctx.set_message(format!("Cleaning up voice (channel {} / {channels})", c + 1));
            let mono: Vec<f32> = (0..frames).map(|i| interleaved[i * channels + c]).collect();
            let x = resample(&mono, rate, SR);
            let y = match &self.model {
                Some(m) => m.enhance(&x, settings, self.chunk, &sub)?,
                None => spectral_gate(&x, settings, &sub)?,
            };
            let mut y = resample(&y, SR, rate);
            y.resize(frames, 0.0);
            for (i, v) in y.into_iter().enumerate() {
                out[i * channels + c] = v;
            }
        }
        Ok(out)
    }

    /// Cleans a WAV file into `out` (16-bit PCM WAV, same rate/channels).
    pub fn process_wav(&self, input: &std::path::Path, out: &std::path::Path, settings: &CleanupSettings, ctx: &JobCtx) -> Result<()> {
        let w = studio_media::wav::read(input)?;
        let y = self.process(&w.samples, w.sample_rate, w.channels as usize, settings, ctx)?;
        let o = studio_media::wav::Wav { sample_rate: w.sample_rate, channels: w.channels, samples: y };
        let tmp = out.with_extension("part");
        std::fs::write(&tmp, studio_media::wav::encode_pcm16(&o))?;
        std::fs::rename(tmp, out)?;
        Ok(())
    }
}

// --------------------------------------------------------------- STFT

/// Streaming analysis/synthesis exactly as DeepFilterNet expects: Vorbis
/// window, spectrum scaled by `2·hop/fft²`, overlap-add with half-window
/// memory. Output lags input by `FFT − HOP` samples.
struct Stft {
    fft: Fft,
    window: Vec<f32>,
    wnorm: f32,
    analysis_mem: Vec<f32>,
    synthesis_mem: Vec<f32>,
}

impl Stft {
    fn new() -> Stft {
        Stft { fft: Fft::new(FFT), window: vorbis_window(FFT), wnorm: (2 * HOP) as f32 / (FFT * FFT) as f32, analysis_mem: vec![0.0; FFT - HOP], synthesis_mem: vec![0.0; FFT - HOP] }
    }

    fn analysis(&mut self, hop: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut buf = Vec::with_capacity(FFT);
        buf.extend_from_slice(&self.analysis_mem);
        buf.extend_from_slice(hop);
        self.analysis_mem.copy_from_slice(&buf[HOP..]);
        for (b, w) in buf.iter_mut().zip(&self.window) {
            *b *= w;
        }
        let (mut re, mut im) = self.fft.rfft(&buf);
        for v in re.iter_mut().chain(im.iter_mut()) {
            *v *= self.wnorm;
        }
        (re, im)
    }

    fn synthesis(&mut self, re: &[f32], im: &[f32]) -> Vec<f32> {
        // nn's inverse divides by n; the reference inverse does not.
        let x = self.fft.irfft(re, im);
        let mut y: Vec<f32> = x.iter().zip(&self.window).map(|(v, w)| v * FFT as f32 * w).collect();
        for (a, m) in y[..FFT - HOP].iter_mut().zip(&self.synthesis_mem) {
            *a += m;
        }
        self.synthesis_mem.copy_from_slice(&y[HOP..]);
        y.truncate(HOP);
        y
    }
}

/// Analyses a whole signal (padded so the STFT delay can be removed).
fn analyse(x: &[f32]) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut stft = Stft::new();
    let mut padded = x.to_vec();
    padded.resize(x.len().div_ceil(HOP) * HOP + FFT, 0.0);
    padded.chunks_exact(HOP).map(|h| stft.analysis(h)).unzip()
}

/// Inverse of [`analyse`], trimmed to `len` samples.
fn synthesise(re: &[Vec<f32>], im: &[Vec<f32>], len: usize) -> Vec<f32> {
    let mut stft = Stft::new();
    let mut y: Vec<f32> = re.iter().zip(im).flat_map(|(r, i)| stft.synthesis(r, i)).collect();
    let delay = FFT - HOP;
    y.drain(..delay.min(y.len()));
    y.resize(len, 0.0);
    y
}

/// Widths (in bins) of the ERB bands.
fn erb_bands() -> Vec<usize> {
    let freq2erb = |f: f32| 9.265 * (1.0 + f / (24.7 * 9.265)).ln();
    let erb2freq = |e: f32| 24.7 * 9.265 * ((e / 9.265).exp() - 1.0);
    let width = SR as f32 / FFT as f32;
    let (lo, hi) = (freq2erb(0.0), freq2erb(SR as f32 / 2.0));
    let step = (hi - lo) / NB_ERB as f32;
    let mut bands = vec![0usize; NB_ERB];
    let (mut prev, mut over) = (0usize, 0usize);
    for (i, b) in bands.iter_mut().enumerate() {
        let fb = (erb2freq(lo + (i + 1) as f32 * step) / width).round() as usize;
        let n = fb.saturating_sub(prev).saturating_sub(over);
        if n < MIN_ERB_FREQS {
            over = MIN_ERB_FREQS - n;
            *b = MIN_ERB_FREQS;
        } else {
            over = 0;
            *b = n;
        }
        prev = fb;
    }
    bands[NB_ERB - 1] += 1;
    let total: usize = bands.iter().sum();
    if total > BINS {
        bands[NB_ERB - 1] -= total - BINS;
    }
    bands
}

fn linspace(a: f32, b: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| a + (b - a) * i as f32 / (n - 1).max(1) as f32).collect()
}

impl Dfn {
    fn enhance(&self, x: &[f32], settings: &CleanupSettings, chunk: usize, ctx: &JobCtx) -> Result<Vec<f32>> {
        let (re, im) = analyse(x);
        let t = re.len();
        let bands = erb_bands();

        // Features with running normalisation, as in training; `tf` frames,
        // the last CONV_LOOKAHEAD of them zero so every real frame gets its
        // lookahead.
        let tf = t + CONV_LOOKAHEAD;
        let mut erb_state = linspace(-60.0, -90.0, NB_ERB);
        let mut unit_state = linspace(0.001, 0.0001, NB_DF);
        let mut feat_erb = vec![0.0f32; tf * NB_ERB];
        let mut feat_spec = vec![0.0f32; 2 * tf * NB_DF];
        for f in 0..t {
            let mut k = 0;
            for (b, &w) in bands.iter().enumerate() {
                let e: f32 = (k..k + w).map(|i| re[f][i] * re[f][i] + im[f][i] * im[f][i]).sum::<f32>() / w as f32;
                k += w;
                let db = 10.0 * (e + 1e-10).log10();
                erb_state[b] = db * (1.0 - NORM_ALPHA) + erb_state[b] * NORM_ALPHA;
                feat_erb[f * NB_ERB + b] = (db - erb_state[b]) / 40.0;
            }
            for i in 0..NB_DF {
                let mag = (re[f][i] * re[f][i] + im[f][i] * im[f][i]).sqrt();
                unit_state[i] = mag * (1.0 - NORM_ALPHA) + unit_state[i] * NORM_ALPHA;
                let s = unit_state[i].sqrt();
                feat_spec[f * NB_DF + i] = re[f][i] / s;
                feat_spec[(tf + f) * NB_DF + i] = im[f][i] / s;
            }
        }
        ctx.set_progress(0.1);
        ctx.check()?;

        // The networks are convolutional/recurrent over time; long files are
        // run in chunks with overlap so memory stays bounded and the
        // recurrent state has time to settle at each chunk start.
        const WARM: usize = 100; // 1 s of context before each chunk
        let mut mask = vec![0.0f32; t * NB_ERB];
        let mut coefs = vec![0.0f32; t * NB_DF * DF_ORDER * 2];
        let cw = NB_DF * DF_ORDER * 2;
        let mut start = 0;
        while start < t {
            ctx.check()?;
            let end = (start + chunk).min(t);
            // network frames a..b cover spectrum frames start..end plus
            // warm-up before and lookahead after
            let a = start.saturating_sub(WARM);
            let b = end + CONV_LOOKAHEAD;
            let n = b - a;
            let fe: Vec<f32> = feat_erb[a * NB_ERB..b * NB_ERB].to_vec();
            let mut fs = Vec::with_capacity(2 * n * NB_DF);
            fs.extend_from_slice(&feat_spec[a * NB_DF..b * NB_DF]);
            fs.extend_from_slice(&feat_spec[(tf + a) * NB_DF..(tf + b) * NB_DF]);
            let mut e = engine::run(&self.enc, vec![("feat_erb", Tensor::f32(&[1, 1, n, NB_ERB], fe)), ("feat_spec", Tensor::f32(&[1, 2, n, NB_DF], fs))])?;
            let (e0, e1, e2, e3, emb, c0) = (e.take("e0")?, e.take("e1")?, e.take("e2")?, e.take("e3")?, e.take("emb")?, e.take("c0")?);
            let mut m = engine::run(&self.erb_dec, vec![("emb", emb.clone()), ("e3", e3), ("e2", e2), ("e1", e1), ("e0", e0)])?;
            let m = m.take("m")?.into_f32_vec();
            let mut d = engine::run(&self.df_dec, vec![("emb", emb), ("c0", c0)])?;
            let c = d.take("coefs")?.into_f32_vec();
            let off = start - a + CONV_LOOKAHEAD;
            let len = end - start;
            mask[start * NB_ERB..end * NB_ERB].copy_from_slice(&m[off * NB_ERB..(off + len) * NB_ERB]);
            coefs[start * cw..end * cw].copy_from_slice(&c[off * cw..(off + len) * cw]);
            start = end;
            ctx.set_progress(0.1 + 0.8 * start as f32 / t as f32);
        }

        // Apply: band gains everywhere, deep filter on the low bins.
        let lim = settings.max_attenuation_db().map(|db| 10f32.powf(-db / 20.0));
        let mut ore = re.clone();
        let mut oim = im.clone();
        for f in 0..t {
            let mut k = 0;
            for (b, &w) in bands.iter().enumerate() {
                let g = mask[f * NB_ERB + b];
                for i in k..k + w {
                    ore[f][i] *= g;
                    oim[f][i] *= g;
                }
                k += w;
            }
            for i in 0..NB_DF {
                let (mut sr, mut si) = (0.0f32, 0.0f32);
                for o in 0..DF_ORDER {
                    // taps cover frames f−2 .. f+2
                    let src = f as isize + o as isize - (DF_ORDER - 1 - DF_LOOKAHEAD) as isize;
                    if src < 0 || src as usize >= t {
                        continue;
                    }
                    let (xr, xi) = (re[src as usize][i], im[src as usize][i]);
                    let ci = ((f * NB_DF + i) * DF_ORDER + o) * 2;
                    let (cr, cim) = (coefs[ci], coefs[ci + 1]);
                    sr += xr * cr - xi * cim;
                    si += xr * cim + xi * cr;
                }
                ore[f][i] = sr;
                oim[f][i] = si;
            }
            if let Some(l) = lim {
                for i in 0..BINS {
                    ore[f][i] = ore[f][i] * (1.0 - l) + re[f][i] * l;
                    oim[f][i] = oim[f][i] * (1.0 - l) + im[f][i] * l;
                }
            }
        }
        ctx.set_progress(0.95);
        Ok(synthesise(&ore, &oim, x.len()))
    }
}

/// Fallback without a model: a spectral gate. The noise floor of each
/// frequency is estimated as a low percentile of its power over time, and
/// a smoothed Wiener-style gain removes what does not rise above it.
pub fn spectral_gate(x: &[f32], settings: &CleanupSettings, ctx: &JobCtx) -> Result<Vec<f32>> {
    let (re, im) = analyse(x);
    let t = re.len();
    if t == 0 {
        return Ok(x.to_vec());
    }
    let power: Vec<Vec<f32>> = re.iter().zip(&im).map(|(r, i)| r.iter().zip(i).map(|(a, b)| a * a + b * b).collect()).collect();
    let mut noise = vec![0.0f32; BINS];
    let mut col = vec![0.0f32; t];
    for (k, n) in noise.iter_mut().enumerate() {
        for f in 0..t {
            col[f] = power[f][k];
        }
        col.sort_by(f32::total_cmp);
        // A noise-only bin's power is roughly exponentially distributed, so
        // its mean is ~4.5× its 20th percentile; the low percentile keeps
        // speech frames out of the estimate.
        *n = col[t / 5] * 4.5;
    }
    ctx.set_progress(0.4);
    let floor = settings.max_attenuation_db().map(|db| 10f32.powf(-db / 20.0)).unwrap_or(0.05).max(0.05);
    let over = 0.5 + settings.strength.clamp(0.0, 1.0);
    let mut ore = re.clone();
    let mut oim = im.clone();
    let mut prev = vec![1.0f32; BINS];
    for f in 0..t {
        ctx.check()?;
        for k in 0..BINS {
            let snr = (power[f][k] - over * noise[k]).max(0.0) / (power[f][k] + 1e-12);
            // smooth over time (fast attack, slower release) to avoid musical noise
            let g = snr.max(floor);
            let g = if g > prev[k] { g } else { 0.6 * prev[k] + 0.4 * g };
            prev[k] = g;
            ore[f][k] *= g;
            oim[f][k] *= g;
        }
    }
    ctx.set_progress(0.9);
    Ok(synthesise(&ore, &oim, x.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stft_round_trip_is_identity() {
        let x: Vec<f32> = (0..10_000).map(|i| (i as f32 * 0.031).sin() * 0.5 + ((i * 7919) % 101) as f32 / 400.0).collect();
        let (re, im) = analyse(&x);
        let y = synthesise(&re, &im, x.len());
        let err = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-4, "max error {err}");
    }

    #[test]
    fn erb_bands_cover_the_spectrum() {
        let b = erb_bands();
        assert_eq!(b.len(), NB_ERB);
        assert_eq!(b.iter().sum::<usize>(), BINS);
        assert!(b.iter().all(|&w| w >= MIN_ERB_FREQS));
        assert!(b[0] <= b[NB_ERB - 1], "bands widen with frequency");
    }

    #[test]
    fn spectral_gate_removes_steady_noise() {
        // a tone that is on half the time, plus white-ish noise
        let n = SR as usize * 2;
        let noise: Vec<f32> = (0..n).map(|i| (((i as u64 * 2_654_435_761) % 1000) as f32 / 1000.0 - 0.5) * 0.05).collect();
        let tone: Vec<f32> = (0..n).map(|i| if i > n / 2 { (i as f32 * 0.05).sin() * 0.3 } else { 0.0 }).collect();
        let x: Vec<f32> = noise.iter().zip(&tone).map(|(a, b)| a + b).collect();
        let y = spectral_gate(&x, &CleanupSettings::default(), &JobCtx::detached()).unwrap();
        let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
        // noise-only half is much quieter, the tone half keeps the tone
        assert!(rms(&y[SR as usize / 4..n / 2 - 4000]) < rms(&x[SR as usize / 4..n / 2 - 4000]) * 0.3);
        assert!(rms(&y[n / 2 + 4000..]) > rms(&tone[n / 2 + 4000..]) * 0.8);
    }

    #[test]
    fn strength_maps_to_attenuation() {
        assert_eq!(CleanupSettings { strength: 1.0 }.max_attenuation_db(), None);
        assert_eq!(CleanupSettings { strength: 0.0 }.max_attenuation_db(), Some(0.0));
        assert_eq!(CleanupSettings { strength: 0.5 }.max_attenuation_db(), Some(18.0));
    }

    #[test]
    fn falls_back_without_the_model() {
        let store = ModelStore::new(std::env::temp_dir().join("studio-ai-no-models"));
        let vc = VoiceCleanup::load(&store).unwrap();
        assert_eq!(vc.engine(), CleanupEngine::SpectralGate);
        let stereo = vec![0.1f32; 4410 * 2];
        let out = vc.process(&stereo, 44_100, 2, &CleanupSettings::default(), &JobCtx::detached()).unwrap();
        assert_eq!(out.len(), stereo.len());
    }
}
