// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MPEG-4 AAC-LC encoder (ISO/IEC 14496-3), mono, stereo or 5.1, 32 / 44.1 / 48 kHz.
//!
//! 5.1 input is interleaved in SMPTE / WAV order `L R C LFE Ls Rs`; the
//! stream uses channel configuration 6 (centre SCE, front and surround CPEs,
//! LFE element), and decoders give the channels back in AAC order
//! `C L R Ls Rs LFE`. The LFE is coded up to about 280 Hz.
//!
//! Every frame is an ONLY_LONG_SEQUENCE with the sine window. Spectra are
//! quantised with one scale factor per channel and frame (white quantisation
//! noise, no psychoacoustic model); a binary search on that global gain fits
//! each frame into the bit budget derived from the target bit rate, with a
//! bit reservoir for difficult frames. Bands above a bit-rate dependent
//! cut-off are not coded. Output is raw access units (no ADTS headers) plus
//! the `AudioSpecificConfig` for an MP4 `esds` box.
//!
//! The decoder output lags the input by [`PRIMING`] samples (the first frame
//! only carries the window overlap); containers signal this with an edit list.

mod huffman_tables;
mod mdct;

use crate::bits::BitWriter;
use huffman_tables as t;
use mdct::{sine_window, Mdct};

/// Samples per frame and channel.
pub const FRAME: usize = 1024;
/// Encoder delay in samples (one frame).
pub const PRIMING: u32 = 1024;
/// Input scale: the decoder reproduces 16-bit sample values from [-1, 1] input,
/// and the standard's analysis MDCT carries a factor of 2.
const INPUT_SCALE: f64 = 2.0 * 32768.0;

static SWB_1024_48: [u16; 50] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160, 176, 196, 216, 240, 264,
    292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704, 736, 768, 800, 832, 864, 896, 928, 1024,
];
static SWB_1024_32: [u16; 52] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160, 176, 196, 216, 240, 264,
    292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704, 736, 768, 800, 832, 864, 896, 928, 960, 992,
    1024,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub sample_rate: u32,
    pub channels: u16,
    /// Target bit rate in bit/s (all channels).
    pub bitrate: u32,
}

/// Track description for a muxer.
#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub asc: Vec<u8>,
    pub bitrate: u32,
    /// Leading samples to hide (edit list media time).
    pub priming: u32,
}

pub struct Encoder {
    cfg: Config,
    sf_index: u8,
    bands: &'static [u16],
    /// Bands coded (bandwidth limit).
    max_sfb: usize,
    /// Bands coded in the LFE channel.
    lfe_max_sfb: usize,
    window: Vec<f64>,
    mdct: Mdct,
    /// Previous frame's input per channel (overlap half of the window).
    prev: Vec<Vec<f64>>,
    /// Interleaved input not yet coded.
    pending: Vec<f32>,
    reservoir: f64,
    mean_bits: f64,
    max_reservoir: f64,
    frames: u64,
    samples_in: u64,
    flushed: bool,
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder, String> {
        let (sf_index, bands): (u8, &'static [u16]) = match cfg.sample_rate {
            48_000 => (3, &SWB_1024_48),
            44_100 => (4, &SWB_1024_48),
            32_000 => (5, &SWB_1024_32),
            r => return Err(format!("AAC encoder: unsupported sample rate {r}")),
        };
        if !matches!(cfg.channels, 1 | 2 | 6) {
            return Err(format!("AAC encoder: unsupported channel count {}", cfg.channels));
        }
        let ch = cfg.channels as usize;
        let bitrate = cfg.bitrate.clamp(8_000 * ch as u32, 320_000 * ch as u32);
        let cfg = Config { bitrate, ..cfg };
        // Bandwidth: about 17 kHz at 128 kbit/s per channel... scaled with the rate.
        let per_ch = bitrate as f64 / ch as f64;
        let cutoff_hz = (5000.0 + per_ch * 0.12).min(cfg.sample_rate as f64 / 2.0).min(20_000.0);
        let cutoff_bin = (cutoff_hz / (cfg.sample_rate as f64 / 2.0) * FRAME as f64) as u16;
        let max_sfb = bands.windows(2).take_while(|w| w[0] < cutoff_bin).count().max(1);
        let mean_bits = bitrate as f64 * FRAME as f64 / cfg.sample_rate as f64;
        // 6144 bits per channel is the decoder input buffer.
        let max_reservoir = (6144.0 * ch as f64 - mean_bits).max(0.0);
        // LFE: the first three bands (12 bins, ~280 Hz at 48 kHz).
        let lfe_max_sfb = 3.min(max_sfb);
        Ok(Encoder {
            cfg,
            sf_index,
            bands,
            max_sfb,
            lfe_max_sfb,
            window: sine_window(2 * FRAME),
            mdct: Mdct::new(FRAME),
            prev: vec![vec![0.0; FRAME]; ch],
            pending: Vec::new(),
            reservoir: 0.0,
            mean_bits,
            max_reservoir,
            frames: 0,
            samples_in: 0,
            flushed: false,
        })
    }

    pub fn config(&self) -> Config {
        self.cfg
    }

    /// AudioSpecificConfig: AAC-LC, sampling frequency index, channel configuration.
    pub fn audio_specific_config(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(5, 2); // audioObjectType: AAC LC
        w.put(4, self.sf_index as u32);
        w.put(4, self.cfg.channels as u32);
        w.put(1, 0); // frameLengthFlag: 1024
        w.put(1, 0); // dependsOnCoreCoder
        w.put(1, 0); // extensionFlag
        w.finish()
    }

    pub fn track_info(&self) -> TrackInfo {
        TrackInfo {
            sample_rate: self.cfg.sample_rate,
            channels: self.cfg.channels,
            asc: self.audio_specific_config(),
            bitrate: self.cfg.bitrate,
            priming: PRIMING,
        }
    }

    /// Input samples per channel accepted so far.
    pub fn samples_in(&self) -> u64 {
        self.samples_in
    }

    pub fn frames_out(&self) -> u64 {
        self.frames
    }

    /// Queue interleaved samples in [-1, 1]; returns every complete access unit.
    pub fn push(&mut self, interleaved: &[f32]) -> Vec<Vec<u8>> {
        let ch = self.cfg.channels as usize;
        self.samples_in += (interleaved.len() / ch) as u64;
        self.pending.extend_from_slice(interleaved);
        let mut out = Vec::new();
        while self.pending.len() >= FRAME * ch {
            let block: Vec<f32> = self.pending.drain(..FRAME * ch).collect();
            out.push(self.encode_frame(&block));
        }
        out
    }

    /// Pad the input with silence and emit the remaining access units,
    /// including the final overlap frame.
    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        if self.flushed {
            return Vec::new();
        }
        self.flushed = true;
        let ch = self.cfg.channels as usize;
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let mut block = std::mem::take(&mut self.pending);
            block.resize(FRAME * ch, 0.0);
            out.push(self.encode_frame(&block));
        }
        out.push(self.encode_frame(&vec![0.0; FRAME * ch]));
        out
    }

    /// Access units needed to carry `samples` per channel (plus priming).
    pub fn frames_for(samples: u64) -> u64 {
        (samples + PRIMING as u64).div_ceil(FRAME as u64)
    }

    fn encode_frame(&mut self, block: &[f32]) -> Vec<u8> {
        let ch = self.cfg.channels as usize;
        // MDCT per channel over [previous frame | this frame].
        let mut spectra = vec![vec![0f64; FRAME]; ch];
        let mut z = vec![0f64; 2 * FRAME];
        for c in 0..ch {
            for n in 0..FRAME {
                z[n] = self.prev[c][n] * self.window[n];
                let x = block[n * ch + c] as f64 * INPUT_SCALE;
                z[FRAME + n] = x * self.window[FRAME + n];
                self.prev[c][n] = x;
            }
            self.mdct.forward(&z, &mut spectra[c]);
        }
        let mag34: Vec<Vec<f64>> = spectra.iter().map(|s| s.iter().map(|v| v.abs().powf(0.75)).collect()).collect();
        let budget = (self.mean_bits + self.reservoir).min(6144.0 * ch as f64 - 64.0);
        // Binary search the smallest (finest) global scale factor that fits.
        let (mut lo, mut hi) = (0i32, 255i32);
        let mut best: Option<Vec<Channel>> = None;
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let chans: Vec<Channel> = spectra.iter().zip(&mag34).enumerate().map(|(c, (s, m))| self.quantise(s, m, mid, self.bands_of(c))).collect();
            let bits = self.frame_bits(&chans);
            if bits as f64 <= budget {
                best = Some(chans);
                hi = mid - 1;
            } else {
                lo = mid + 1;
            }
        }
        let chans = best.unwrap_or_else(|| spectra.iter().zip(&mag34).enumerate().map(|(c, (s, m))| self.quantise(s, m, 255, self.bands_of(c))).collect());
        let au = self.write_frame(&chans);
        let used = au.len() as f64 * 8.0;
        self.reservoir = (self.reservoir + self.mean_bits - used).clamp(0.0, self.max_reservoir);
        self.frames += 1;
        au
    }

    /// Quantise a spectrum with global scale factor `g` (raised per band where
    /// the escape code book's range would overflow).
    fn quantise(&self, spec: &[f64], mag34: &[f64], g: i32, max_sfb: usize) -> Channel {
        let mut ch = Channel { q: vec![0; FRAME], sf: vec![0; max_sfb], cb: vec![0; max_sfb] };
        for b in 0..max_sfb {
            let (s, e) = (self.bands[b] as usize, self.bands[b + 1] as usize);
            let peak = spec[s..e].iter().fold(0f64, |m, v| m.max(v.abs()));
            let mut sf = g;
            // |q| <= 8191: raise the band's scale factor if needed.
            while sf < 255 && (peak * 2f64.powf(-0.25 * (sf as f64 - 100.0))).powf(0.75) + 0.4054 >= 8191.0 {
                sf += 1;
            }
            let gain34 = 2f64.powf(-0.1875 * (sf as f64 - 100.0));
            let mut maxq = 0;
            for k in s..e {
                let a = (mag34[k] * gain34 + 0.4054).floor() as i32;
                let a = a.min(8191);
                ch.q[k] = if spec[k] < 0.0 { -a } else { a };
                maxq = maxq.max(a);
            }
            ch.sf[b] = sf;
            ch.cb[b] = if maxq == 0 { 0 } else { best_book(&ch.q[s..e], maxq).0 };
        }
        // Consecutive coded scale factors may differ by at most 60.
        let mut last: Option<i32> = None;
        for b in 0..max_sfb {
            if ch.cb[b] == 0 {
                continue;
            }
            if let Some(l) = last {
                let c = ch.sf[b].clamp(l - 60, l + 60);
                if c != ch.sf[b] {
                    ch.sf[b] = c;
                    self.requant_band(&mut ch, b, spec);
                    if ch.cb[b] == 0 {
                        continue;
                    }
                }
            }
            last = Some(ch.sf[b]);
        }
        ch
    }

    fn requant_band(&self, ch: &mut Channel, b: usize, spec: &[f64]) {
        let (s, e) = (self.bands[b] as usize, self.bands[b + 1] as usize);
        let gain = 2f64.powf(-0.25 * (ch.sf[b] as f64 - 100.0));
        let mut maxq = 0;
        for k in s..e {
            let a = (((spec[k].abs() * gain).powf(0.75) + 0.4054).floor() as i32).min(8191);
            ch.q[k] = if spec[k] < 0.0 { -a } else { a };
            maxq = maxq.max(a);
        }
        ch.cb[b] = if maxq == 0 { 0 } else { best_book(&ch.q[s..e], maxq).0 };
    }

    /// Bands coded for input channel `c`.
    fn bands_of(&self, c: usize) -> usize {
        if self.cfg.channels == 6 && c == LFE_INPUT { self.lfe_max_sfb } else { self.max_sfb }
    }

    fn frame_bits(&self, chans: &[Channel]) -> u32 {
        // Element headers + END + byte alignment upper bound.
        let mut bits = 3 + 7;
        for e in elements(self.cfg.channels) {
            bits += 3 + 4 + if matches!(e, Element::Cpe(..)) { 1 } else { 0 };
        }
        for c in chans {
            bits += self.ics_bits(c);
        }
        bits
    }

    fn ics_bits(&self, c: &Channel) -> u32 {
        let mut bits = 8 + 1 + 2 + 1 + 6 + 1 + 3; // global_gain, ics_info, pulse/tns/gain flags
        for (cb, len) in sections(&c.cb) {
            bits += 4 + 5 * (len as u32 / 31 + 1);
            let _ = cb;
        }
        let mut last = first_sf(c);
        for b in 0..c.cb.len() {
            if c.cb[b] == 0 {
                continue;
            }
            let d = (c.sf[b] - last + 60) as usize;
            bits += t::SF_LENS[d] as u32;
            last = c.sf[b];
            let (s, e) = (self.bands[b] as usize, self.bands[b + 1] as usize);
            bits += book_bits(c.cb[b], &c.q[s..e]);
        }
        bits
    }

    fn write_frame(&self, chans: &[Channel]) -> Vec<u8> {
        let mut w = BitWriter::with_capacity(1024 * chans.len());
        for &e in elements(self.cfg.channels) {
            match e {
                Element::Sce(c, tag) => {
                    w.put(3, 0); // SCE
                    w.put(4, tag);
                    self.write_ics(&mut w, &chans[c]);
                }
                Element::Cpe(a, b, tag) => {
                    w.put(3, 1); // CPE
                    w.put(4, tag);
                    w.put(1, 0); // common_window
                    self.write_ics(&mut w, &chans[a]);
                    self.write_ics(&mut w, &chans[b]);
                }
                Element::Lfe(c, tag) => {
                    w.put(3, 3); // LFE
                    w.put(4, tag);
                    self.write_ics(&mut w, &chans[c]);
                }
            }
        }
        w.put(3, 7); // END
        w.finish()
    }

    fn write_ics(&self, w: &mut BitWriter, c: &Channel) {
        let gg = first_sf(c);
        w.put(8, gg as u32);
        // ics_info
        w.put(1, 0); // ics_reserved_bit
        w.put(2, 0); // ONLY_LONG_SEQUENCE
        w.put(1, 0); // sine window
        let max_sfb = c.cb.len();
        w.put(6, max_sfb as u32);
        w.put(1, 0); // predictor_data_present
        // section_data
        for (cb, len) in sections(&c.cb) {
            w.put(4, cb as u32);
            let mut l = len;
            while l >= 31 {
                w.put(5, 31);
                l -= 31;
            }
            w.put(5, l as u32);
        }
        // scale_factor_data
        let mut last = gg;
        for b in 0..max_sfb {
            if c.cb[b] == 0 {
                continue;
            }
            let d = (c.sf[b] - last + 60) as usize;
            w.put(t::SF_LENS[d] as u32, t::SF_CODES[d]);
            last = c.sf[b];
        }
        w.put(1, 0); // pulse_data_present
        w.put(1, 0); // tns_data_present
        w.put(1, 0); // gain_control_data_present
        // spectral_data
        for b in 0..max_sfb {
            if c.cb[b] == 0 {
                continue;
            }
            let (s, e) = (self.bands[b] as usize, self.bands[b + 1] as usize);
            write_book(w, c.cb[b], &c.q[s..e]);
        }
    }
}

/// Input index of the LFE channel in 5.1 (`L R C LFE Ls Rs`).
const LFE_INPUT: usize = 3;

/// A syntactic element and the input channels it carries (with its tag).
#[derive(Clone, Copy, Debug)]
enum Element {
    Sce(usize, u32),
    Cpe(usize, usize, u32),
    Lfe(usize, u32),
}

/// Elements in bitstream order for a channel count (configuration 1, 2, 6).
fn elements(channels: u16) -> &'static [Element] {
    match channels {
        1 => &[Element::Sce(0, 0)],
        6 => &[Element::Sce(2, 0), Element::Cpe(0, 1, 0), Element::Cpe(4, 5, 1), Element::Lfe(3, 0)],
        _ => &[Element::Cpe(0, 1, 0)],
    }
}

/// Quantised spectrum of one channel.
struct Channel {
    q: Vec<i32>,
    sf: Vec<i32>,
    /// Code book per band (0 = ZERO_HCB).
    cb: Vec<u8>,
}

fn first_sf(c: &Channel) -> i32 {
    c.cb.iter().position(|&b| b != 0).map_or(100, |b| c.sf[b])
}

/// Runs of equal code books: (book, number of bands).
fn sections(cb: &[u8]) -> Vec<(u8, usize)> {
    let mut out: Vec<(u8, usize)> = Vec::new();
    for &b in cb {
        match out.last_mut() {
            Some((c, n)) if *c == b => *n += 1,
            _ => out.push((b, 1)),
        }
    }
    out
}

fn spec_table(cb: u8) -> (&'static [u8], &'static [u32]) {
    match cb {
        1 => (&t::SPEC1_LENS, &t::SPEC1_CODES),
        2 => (&t::SPEC2_LENS, &t::SPEC2_CODES),
        3 => (&t::SPEC3_LENS, &t::SPEC3_CODES),
        4 => (&t::SPEC4_LENS, &t::SPEC4_CODES),
        5 => (&t::SPEC5_LENS, &t::SPEC5_CODES),
        6 => (&t::SPEC6_LENS, &t::SPEC6_CODES),
        7 => (&t::SPEC7_LENS, &t::SPEC7_CODES),
        8 => (&t::SPEC8_LENS, &t::SPEC8_CODES),
        9 => (&t::SPEC9_LENS, &t::SPEC9_CODES),
        10 => (&t::SPEC10_LENS, &t::SPEC10_CODES),
        _ => (&t::SPEC11_LENS, &t::SPEC11_CODES),
    }
}

/// (dimension, signed, modulo, offset) of spectral book `cb`.
fn shape(cb: u8) -> (usize, bool, i32, i32) {
    match cb {
        1 | 2 => (4, true, 3, 1),
        3 | 4 => (4, false, 3, 0),
        5 | 6 => (2, true, 9, 4),
        7 | 8 => (2, false, 8, 0),
        9 | 10 => (2, false, 13, 0),
        _ => (2, false, 17, 0),
    }
}

/// Largest absolute value a book codes directly.
fn lav(cb: u8) -> i32 {
    match cb {
        1 | 2 => 1,
        3 | 4 => 2,
        5 | 6 => 4,
        7 | 8 => 7,
        9 | 10 => 12,
        _ => 16,
    }
}

fn escape_bits(v: i32) -> u32 {
    if v < 16 {
        return 0;
    }
    let n = (31 - (v as u32).leading_zeros()) - 4;
    n + 1 + n + 4
}

/// Bits to code `q` (one band) with book `cb`.
fn book_bits(cb: u8, q: &[i32]) -> u32 {
    let (dim, signed, modulo, off) = shape(cb);
    let (lens, _) = spec_table(cb);
    let mut bits = 0;
    for tuple in q.chunks(dim) {
        let mut idx = 0i32;
        for &v in tuple {
            let a = if signed { v } else { v.abs().min(16) };
            idx = idx * modulo + a + off;
            if !signed && v != 0 {
                bits += 1;
            }
            if cb == 11 {
                bits += escape_bits(v.abs());
            }
        }
        bits += lens[idx as usize] as u32;
    }
    bits
}

fn write_book(w: &mut BitWriter, cb: u8, q: &[i32]) {
    let (dim, signed, modulo, off) = shape(cb);
    let (lens, codes) = spec_table(cb);
    for tuple in q.chunks(dim) {
        let mut idx = 0i32;
        for &v in tuple {
            let a = if signed { v } else { v.abs().min(16) };
            idx = idx * modulo + a + off;
        }
        w.put(lens[idx as usize] as u32, codes[idx as usize]);
        if !signed {
            for &v in tuple {
                if v != 0 {
                    w.put_bit(v < 0);
                }
            }
        }
        if cb == 11 {
            for &v in tuple {
                let a = v.abs();
                if a >= 16 {
                    let n = (31 - (a as u32).leading_zeros()) - 4;
                    w.put(n + 1, ((1u32 << n) - 1) << 1); // n ones then a zero
                    w.put(n + 4, a as u32 - (1 << (n + 4)));
                }
            }
        }
    }
}

/// Cheapest book able to code a band whose largest magnitude is `maxq`.
fn best_book(q: &[i32], maxq: i32) -> (u8, u32) {
    let mut best = (11u8, u32::MAX);
    for cb in 1..=11u8 {
        if lav(cb) < maxq && cb != 11 {
            continue;
        }
        let b = book_bits(cb, q);
        if b < best.1 {
            best = (cb, b);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asc_bytes() {
        let e = Encoder::new(Config { sample_rate: 48_000, channels: 2, bitrate: 128_000 }).unwrap();
        assert_eq!(e.audio_specific_config(), vec![0x11, 0x90]);
        let e = Encoder::new(Config { sample_rate: 44_100, channels: 1, bitrate: 64_000 }).unwrap();
        assert_eq!(e.audio_specific_config(), vec![0x12, 0x08]);
    }

    #[test]
    fn book_bits_match_written_bits() {
        let q = [0, 1, -2, 3, 17, -40, 5, 0, 0, 0, 1, -1, 12, 0, 300, -8191];
        for cb in 1..=11u8 {
            let maxq = q.iter().map(|v: &i32| v.abs()).max().unwrap();
            let band: Vec<i32> = if lav(cb) >= maxq { q.to_vec() } else { q.iter().map(|&v| v.clamp(-lav(cb), lav(cb))).collect() };
            let mut w = BitWriter::new();
            write_book(&mut w, cb, &band);
            assert_eq!(w.bit_len() as u32, book_bits(cb, &band), "book {cb}");
        }
    }
}
