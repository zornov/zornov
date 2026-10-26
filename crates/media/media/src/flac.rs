// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FLAC (Free Lossless Audio Codec) writing and reading, 16-bit PCM.
//!
//! The encoder streams fixed 4096-sample blocks. For every block it tries
//! the four stereo decorrelations (independent, left/side, side/right,
//! mid/side) and, per channel, a constant, the fixed polynomial predictors
//! of order 0–4 and quantised LPC of order 1–8 (Welch-windowed
//! autocorrelation + Levinson–Durbin), each with partitioned Rice coding of
//! the residual, and keeps the smallest. STREAMINFO (frame sizes, sample
//! count, MD5 of the PCM) is patched in when the stream is finished.
//!
//! The decoder reads any FLAC stream with 4–24-bit samples, checking every
//! frame's CRC; it exists for tests and for importing `.flac` files.

use crate::bits::BitWriter;
use std::io::{self, Seek, SeekFrom, Write};

pub const BLOCK: usize = 4096;
const MAX_LPC: usize = 8;
const QLP_PRECISION: u32 = 12;
const MAX_PARTITION_ORDER: u32 = 6;

// --- checksums -------------------------------------------------------------------

fn crc8(data: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in data {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

fn crc16(data: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in data {
        c ^= u16::from(b) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

/// MD5 (RFC 1321), streaming.
pub struct Md5 {
    state: [u32; 4],
    buf: Vec<u8>,
    len: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self { state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476], buf: Vec::with_capacity(64), len: 0 }
    }
}

impl Md5 {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
        21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];

    fn block(&mut self, chunk: &[u8]) {
        let m: Vec<u32> = chunk.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let k = ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32;
            let f = f.wrapping_add(a).wrapping_add(k).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(Self::S[i]));
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.buf.is_empty() {
            let take = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let b = std::mem::take(&mut self.buf);
                self.block(&b);
                self.buf = b;
                self.buf.clear();
            }
        }
        let mut chunks = data.chunks_exact(64);
        for c in &mut chunks {
            self.block(c);
        }
        self.buf.extend_from_slice(chunks.remainder());
    }

    pub fn finish(mut self) -> [u8; 16] {
        let bits = self.len.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (self.len + pad.len() as u64) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_le_bytes());
        let len = self.len;
        self.update(&pad);
        self.len = len;
        let mut out = [0u8; 16];
        for (i, s) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        out
    }
}

// --- residual coding --------------------------------------------------------------

fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

/// Best Rice parameter for a partition with sum `s` of zigzagged values over `n`.
fn rice_param(s: u64, n: usize) -> u32 {
    if n == 0 || s == 0 {
        return 0;
    }
    let mean = s / n as u64;
    if mean == 0 { 0 } else { (64 - mean.leading_zeros()).min(30) }
}

fn rice_bits(s: u64, n: usize, k: u32) -> u64 {
    n as u64 * (u64::from(k) + 1) + (s >> k)
}

/// Chooses a partition order and parameters; returns (estimated bits, order, params).
fn plan_residual(res: &[i32], pred_order: usize, block: usize) -> (u64, u32, Vec<u32>) {
    let u: Vec<u64> = res.iter().map(|&r| u64::from(zigzag(r))).collect();
    let mut best: Option<(u64, u32, Vec<u32>)> = None;
    for order in 0..=MAX_PARTITION_ORDER {
        let parts = 1usize << order;
        if block % parts != 0 || block >> order <= pred_order {
            break;
        }
        let mut bits = 6u64; // method + order
        let mut params = Vec::with_capacity(parts);
        let mut at = 0;
        for p in 0..parts {
            let n = (block >> order) - if p == 0 { pred_order } else { 0 };
            let s: u64 = u[at..at + n].iter().sum();
            at += n;
            let k0 = rice_param(s, n);
            let (k, b) = [k0.saturating_sub(1), k0, (k0 + 1).min(30)].into_iter().map(|k| (k, rice_bits(s, n, k))).min_by_key(|x| x.1).unwrap();
            bits += b + if k > 14 { 5 } else { 4 };
            params.push(k);
        }
        if best.as_ref().is_none_or(|b| bits < b.0) {
            best = Some((bits, order, params));
        }
    }
    best.unwrap_or((u64::MAX, 0, vec![0]))
}

fn write_residual(w: &mut BitWriter, res: &[i32], pred_order: usize, block: usize, order: u32, params: &[u32]) {
    let wide = params.iter().any(|&k| k > 14);
    w.put(u32::from(wide), 2);
    w.put(order, 4);
    let mut at = 0;
    for (p, &k) in params.iter().enumerate() {
        let n = (block >> order) - if p == 0 { pred_order } else { 0 };
        w.put(k, if wide { 5 } else { 4 });
        for &r in &res[at..at + n] {
            let u = zigzag(r);
            let mut q = u >> k;
            while q >= 32 {
                w.put(0, 32);
                q -= 32;
            }
            w.put(1, q + 1);
            if k > 0 {
                w.put(u & ((1u32 << k) - 1), k);
            }
        }
        at += n;
    }
}

// --- prediction ---------------------------------------------------------------------

fn fixed_residual(x: &[i32], order: usize) -> Vec<i32> {
    (order..x.len())
        .map(|i| {
            let p = match order {
                0 => 0,
                1 => i64::from(x[i - 1]),
                2 => 2 * i64::from(x[i - 1]) - i64::from(x[i - 2]),
                3 => 3 * i64::from(x[i - 1]) - 3 * i64::from(x[i - 2]) + i64::from(x[i - 3]),
                _ => 4 * i64::from(x[i - 1]) - 6 * i64::from(x[i - 2]) + 4 * i64::from(x[i - 3]) - i64::from(x[i - 4]),
            };
            (i64::from(x[i]) - p) as i32
        })
        .collect()
}

fn lpc_residual(x: &[i32], q: &[i32], shift: u32) -> Vec<i32> {
    let order = q.len();
    (order..x.len())
        .map(|i| {
            let mut s = 0i64;
            for (j, &c) in q.iter().enumerate() {
                s += i64::from(c) * i64::from(x[i - j - 1]);
            }
            (i64::from(x[i]) - (s >> shift)) as i32
        })
        .collect()
}

/// LPC coefficient sets for orders 1..=max (Levinson–Durbin on a Welch window).
fn lpc_coefficients(x: &[i32], max: usize) -> Vec<Vec<f64>> {
    let n = x.len();
    if n <= max + 1 {
        return Vec::new();
    }
    let win: Vec<f64> = (0..n)
        .map(|i| {
            let t = (2.0 * i as f64 - (n - 1) as f64) / (n + 1) as f64;
            f64::from(x[i]) * (1.0 - t * t)
        })
        .collect();
    let mut r = vec![0f64; max + 1];
    for (lag, rv) in r.iter_mut().enumerate() {
        *rv = (lag..n).map(|i| win[i] * win[i - lag]).sum();
    }
    if r[0] <= 0.0 {
        return Vec::new();
    }
    r[0] *= 1.0 + 1e-9;
    let mut out = Vec::new();
    let mut a = vec![0f64; max + 1];
    let mut err = r[0];
    for m in 1..=max {
        let mut acc = r[m];
        for j in 1..m {
            acc -= a[j] * r[m - j];
        }
        let k = acc / err;
        let prev = a.clone();
        a[m] = k;
        for j in 1..m {
            a[j] = prev[j] - k * prev[m - j];
        }
        err *= 1.0 - k * k;
        out.push(a[1..=m].to_vec());
        if err <= 0.0 {
            break;
        }
    }
    out
}

/// Quantises coefficients to `precision` bits; returns (coefs, shift).
fn quantize(lp: &[f64], precision: u32) -> Option<(Vec<i32>, u32)> {
    let cmax = lp.iter().fold(0f64, |m, c| m.max(c.abs()));
    if cmax <= 0.0 || !cmax.is_finite() {
        return None;
    }
    let log2 = cmax.log2().floor() as i32 + 1;
    let shift = (precision as i32 - 1 - log2).clamp(0, 15) as u32;
    let lim = (1i32 << (precision - 1)) - 1;
    let mut err = 0f64;
    let q = lp
        .iter()
        .map(|&c| {
            let v = c * f64::from(1u32 << shift) + err;
            let r = v.round().clamp(-f64::from(lim) - 1.0, f64::from(lim));
            err = v - r;
            r as i32
        })
        .collect();
    Some((q, shift))
}

enum Sub {
    Constant(i32),
    Verbatim,
    Fixed(usize, Vec<i32>, u32, Vec<u32>),
    Lpc(Vec<i32>, u32, Vec<i32>, u32, Vec<u32>),
}

/// The smallest subframe for one channel: (bits, choice).
fn best_subframe(x: &[i32], bps: u32) -> (u64, Sub) {
    let n = x.len();
    if x.iter().all(|&v| v == x[0]) {
        return (8 + u64::from(bps), Sub::Constant(x[0]));
    }
    let mut best = (8 + n as u64 * u64::from(bps), Sub::Verbatim);
    for order in 0..=4.min(n - 1) {
        let res = fixed_residual(x, order);
        let (bits, po, params) = plan_residual(&res, order, n);
        let total = 8 + order as u64 * u64::from(bps) + bits;
        if total < best.0 {
            best = (total, Sub::Fixed(order, res, po, params));
        }
    }
    for lp in lpc_coefficients(x, MAX_LPC.min(n.saturating_sub(1))) {
        let Some((q, shift)) = quantize(&lp, QLP_PRECISION) else { continue };
        let order = q.len();
        let res = lpc_residual(x, &q, shift);
        // Residuals must fit the 32-bit Rice path.
        if res.iter().any(|&r| r.unsigned_abs() > 1 << 30) {
            continue;
        }
        let (bits, po, params) = plan_residual(&res, order, n);
        let total = 8 + order as u64 * u64::from(bps) + 4 + 5 + order as u64 * u64::from(QLP_PRECISION) + bits;
        if total < best.0 {
            best = (total, Sub::Lpc(q, shift, res, po, params));
        }
    }
    best
}

fn signed(v: i32, bits: u32) -> u32 {
    (v as u32) & (u32::MAX >> (32 - bits))
}

fn write_subframe(w: &mut BitWriter, x: &[i32], bps: u32, sub: &Sub) {
    let n = x.len();
    match sub {
        Sub::Constant(v) => {
            w.put(0, 8);
            w.put(signed(*v, bps), bps);
        }
        Sub::Verbatim => {
            w.put(0b0000_0010, 8);
            for &v in x {
                w.put(signed(v, bps), bps);
            }
        }
        Sub::Fixed(order, res, po, params) => {
            w.put((0b001000 | *order as u32) << 1, 8);
            for &v in &x[..*order] {
                w.put(signed(v, bps), bps);
            }
            write_residual(w, res, *order, n, *po, params);
        }
        Sub::Lpc(q, shift, res, po, params) => {
            let order = q.len();
            w.put((0b100000 | (order as u32 - 1)) << 1, 8);
            for &v in &x[..order] {
                w.put(signed(v, bps), bps);
            }
            w.put(QLP_PRECISION - 1, 4);
            w.put(*shift, 5);
            for &c in q {
                w.put(signed(c, QLP_PRECISION), QLP_PRECISION);
            }
            write_residual(w, res, order, n, *po, params);
        }
    }
}

fn utf8_number(mut v: u64, out: &mut Vec<u8>) {
    if v < 0x80 {
        out.push(v as u8);
        return;
    }
    let mut tail = Vec::new();
    let mut lead_bits = 6; // payload bits left in the lead byte
    while v >= 1 << lead_bits {
        tail.push(0x80 | (v & 0x3F) as u8);
        v >>= 6;
        lead_bits -= 1;
    }
    let n = tail.len() + 1;
    let mark = (0xFFu16 << (8 - n)) as u8;
    out.push(mark | v as u8);
    out.extend(tail.iter().rev());
}

fn rate_code(rate: u32) -> u32 {
    match rate {
        88_200 => 0b0001,
        176_400 => 0b0010,
        192_000 => 0b0011,
        8_000 => 0b0100,
        16_000 => 0b0101,
        22_050 => 0b0110,
        24_000 => 0b0111,
        32_000 => 0b1000,
        44_100 => 0b1001,
        48_000 => 0b1010,
        96_000 => 0b1011,
        _ => 0,
    }
}

/// Encodes one frame of `channels` × n samples (16-bit).
pub fn encode_frame(chans: &[Vec<i32>], rate: u32, number: u64) -> Vec<u8> {
    let n = chans[0].len();
    let stereo = chans.len() == 2;
    // Candidate (assignment code, [(signal, bps)]).
    let mut cands: Vec<(u32, Vec<(Vec<i32>, u32)>)> = vec![(chans.len() as u32 - 1, chans.iter().map(|c| (c.clone(), 16)).collect())];
    if stereo {
        let (l, r) = (&chans[0], &chans[1]);
        let side: Vec<i32> = l.iter().zip(r).map(|(a, b)| a - b).collect();
        let mid: Vec<i32> = l.iter().zip(r).map(|(a, b)| (a + b) >> 1).collect();
        cands.push((0b1000, vec![(l.clone(), 16), (side.clone(), 17)]));
        cands.push((0b1001, vec![(side.clone(), 17), (r.clone(), 16)]));
        cands.push((0b1010, vec![(mid, 16), (side, 17)]));
    }
    // Each distinct signal is analysed once.
    let mut cache: Vec<(Vec<i32>, u32, u64, Sub)> = Vec::new();
    let mut best: Option<(u64, u32, Vec<usize>)> = None;
    for (code, sigs) in &cands {
        let mut idx = Vec::with_capacity(sigs.len());
        for (sig, bps) in sigs {
            let i = match cache.iter().position(|c| c.1 == *bps && &c.0 == sig) {
                Some(i) => i,
                None => {
                    let (bits, sub) = best_subframe(sig, *bps);
                    cache.push((sig.clone(), *bps, bits, sub));
                    cache.len() - 1
                }
            };
            idx.push(i);
        }
        let bits: u64 = idx.iter().map(|&i| cache[i].2).sum();
        if best.as_ref().is_none_or(|b| bits < b.0) {
            best = Some((bits, *code, idx));
        }
    }
    let (_, code, idx) = best.unwrap();

    let mut head = vec![0xFF, 0xF8];
    let (bs_code, bs_tail) = match n {
        4096 => (0b1100, None),
        n if n <= 256 => (0b0110, Some(vec![(n - 1) as u8])),
        n => (0b0111, Some(((n - 1) as u16).to_be_bytes().to_vec())),
    };
    let rc = rate_code(rate);
    head.push((bs_code << 4 | rc) as u8);
    head.push((code << 4 | 0b100 << 1) as u8);
    utf8_number(number, &mut head);
    if let Some(t) = bs_tail {
        head.extend(t);
    }
    head.push(crc8(&head));
    let mut w = BitWriter::new();
    for &b in &head {
        w.put(u32::from(b), 8);
    }
    for &i in &idx {
        let (sig, bps, _, sub) = &cache[i];
        write_subframe(&mut w, sig, *bps, sub);
    }
    let mut bytes = w.finish();
    let crc = crc16(&bytes);
    bytes.extend_from_slice(&crc.to_be_bytes());
    bytes
}

fn streaminfo(rate: u32, channels: u16, min_fs: u32, max_fs: u32, total: u64, md5: [u8; 16], last_block: usize) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.put(1, 1); // last metadata block
    w.put(0, 7); // STREAMINFO
    w.put(34, 24);
    let min_bs = if total > BLOCK as u64 { BLOCK } else { last_block.max(16) };
    w.put(min_bs.min(BLOCK) as u32, 16);
    w.put(BLOCK as u32, 16);
    w.put(min_fs, 24);
    w.put(max_fs, 24);
    w.put(rate, 20);
    w.put(u32::from(channels) - 1, 3);
    w.put(15, 5);
    w.put((total >> 32) as u32 & 0xF, 4);
    w.put(total as u32, 32);
    let mut b = w.finish();
    b.extend_from_slice(&md5);
    b
}

/// Streams 16-bit PCM into a FLAC file.
pub struct FlacWriter<W: Write + Seek> {
    w: W,
    rate: u32,
    channels: u16,
    buf: Vec<Vec<i32>>,
    frame: u64,
    total: u64,
    min_fs: u32,
    max_fs: u32,
    md5: Md5,
    last_block: usize,
}

impl<W: Write + Seek> FlacWriter<W> {
    pub fn new(mut w: W, rate: u32, channels: u16) -> io::Result<Self> {
        if !(1..=8).contains(&channels) || rate == 0 || rate >= 1 << 20 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsupported FLAC format"));
        }
        w.write_all(b"fLaC")?;
        w.write_all(&streaminfo(rate, channels, 0, 0, 0, [0; 16], BLOCK))?;
        Ok(Self { w, rate, channels, buf: vec![Vec::with_capacity(BLOCK); channels as usize], frame: 0, total: 0, min_fs: u32::MAX, max_fs: 0, md5: Md5::default(), last_block: BLOCK })
    }

    /// Interleaved samples in [-1, 1], quantised like the WAV writer.
    pub fn push_f32(&mut self, samples: &[f32]) -> io::Result<()> {
        let v: Vec<i16> = samples.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect();
        self.push_i16(&v)
    }

    pub fn push_i16(&mut self, samples: &[i16]) -> io::Result<()> {
        let ch = self.channels as usize;
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for (i, &s) in samples.iter().enumerate() {
            bytes.extend_from_slice(&s.to_le_bytes());
            self.buf[i % ch].push(i32::from(s));
            if i % ch == ch - 1 && self.buf[0].len() == BLOCK {
                self.flush_block()?;
            }
        }
        self.md5.update(&bytes);
        Ok(())
    }

    fn flush_block(&mut self) -> io::Result<()> {
        let n = self.buf[0].len();
        if n == 0 {
            return Ok(());
        }
        let bytes = encode_frame(&self.buf, self.rate, self.frame);
        self.w.write_all(&bytes)?;
        self.min_fs = self.min_fs.min(bytes.len() as u32);
        self.max_fs = self.max_fs.max(bytes.len() as u32);
        self.frame += 1;
        self.total += n as u64;
        self.last_block = n;
        for c in &mut self.buf {
            c.clear();
        }
        Ok(())
    }

    /// Writes the last partial block and patches STREAMINFO.
    pub fn finish(mut self) -> io::Result<W> {
        // A trailing partial frame, keeping full channels only.
        let full = self.buf.iter().map(Vec::len).min().unwrap_or(0);
        for c in &mut self.buf {
            c.truncate(full);
        }
        self.flush_block()?;
        let md5 = std::mem::take(&mut self.md5).finish();
        let min_fs = if self.min_fs == u32::MAX { 0 } else { self.min_fs };
        self.w.seek(SeekFrom::Start(4))?;
        self.w.write_all(&streaminfo(self.rate, self.channels, min_fs, self.max_fs, self.total, md5, self.last_block))?;
        self.w.seek(SeekFrom::End(0))?;
        self.w.flush()?;
        Ok(self.w)
    }
}

// --- decoder ------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Flac {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u32,
    pub total_samples: u64,
    pub md5: [u8; 16],
    /// Interleaved integer samples.
    pub samples: Vec<i32>,
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize, // bits
}

impl Reader<'_> {
    fn bits(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = *self.b.get(self.pos >> 3).ok_or("FLAC stream ends early")?;
            v = (v << 1) | u64::from((byte >> (7 - (self.pos & 7))) & 1);
            self.pos += 1;
        }
        Ok(v as u32)
    }
    fn sbits(&mut self, n: u32) -> Result<i32, String> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.bits(n)?;
        Ok(((v << (32 - n)) as i32) >> (32 - n))
    }
    fn unary(&mut self) -> Result<u32, String> {
        let mut q = 0;
        while self.bits(1)? == 0 {
            q += 1;
        }
        Ok(q)
    }
    fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }
}

fn decode_residual(r: &mut Reader, n: usize, order: usize, out: &mut Vec<i32>) -> Result<(), String> {
    let method = r.bits(2)?;
    if method > 1 {
        return Err("reserved FLAC residual method".into());
    }
    let pbits = if method == 1 { 5 } else { 4 };
    let po = r.bits(4)?;
    let parts = 1usize << po;
    for p in 0..parts {
        let cnt = (n >> po).checked_sub(if p == 0 { order } else { 0 }).ok_or("bad FLAC partition")?;
        let k = r.bits(pbits)?;
        if k == (1 << pbits) - 1 {
            let raw = r.bits(5)?;
            for _ in 0..cnt {
                out.push(r.sbits(raw)?);
            }
        } else {
            for _ in 0..cnt {
                let u = (r.unary()? << k) | r.bits(k)?;
                out.push(((u >> 1) as i32) ^ -((u & 1) as i32));
            }
        }
    }
    Ok(())
}

fn decode_subframe(r: &mut Reader, n: usize, bps: u32) -> Result<Vec<i32>, String> {
    if r.bits(1)? != 0 {
        return Err("bad FLAC subframe padding".into());
    }
    let t = r.bits(6)?;
    let mut wasted = 0;
    if r.bits(1)? == 1 {
        wasted = r.unary()? + 1;
    }
    let bps = bps - wasted;
    let mut x = Vec::with_capacity(n);
    match t {
        0 => {
            let v = r.sbits(bps)?;
            x.resize(n, v);
        }
        1 => {
            for _ in 0..n {
                x.push(r.sbits(bps)?);
            }
        }
        8..=12 => {
            let order = (t - 8) as usize;
            for _ in 0..order {
                x.push(r.sbits(bps)?);
            }
            let mut res = Vec::with_capacity(n);
            decode_residual(r, n, order, &mut res)?;
            for e in res {
                let i = x.len();
                let p: i64 = match order {
                    0 => 0,
                    1 => i64::from(x[i - 1]),
                    2 => 2 * i64::from(x[i - 1]) - i64::from(x[i - 2]),
                    3 => 3 * i64::from(x[i - 1]) - 3 * i64::from(x[i - 2]) + i64::from(x[i - 3]),
                    _ => 4 * i64::from(x[i - 1]) - 6 * i64::from(x[i - 2]) + 4 * i64::from(x[i - 3]) - i64::from(x[i - 4]),
                };
                x.push((p + i64::from(e)) as i32);
            }
        }
        32..=63 => {
            let order = (t - 31) as usize;
            for _ in 0..order {
                x.push(r.sbits(bps)?);
            }
            let prec = r.bits(4)? + 1;
            if prec == 16 {
                return Err("bad FLAC LPC precision".into());
            }
            let shift = r.sbits(5)?;
            if shift < 0 {
                return Err("negative FLAC LPC shift".into());
            }
            let q: Vec<i64> = (0..order).map(|_| r.sbits(prec).map(i64::from)).collect::<Result<_, _>>()?;
            let mut res = Vec::with_capacity(n);
            decode_residual(r, n, order, &mut res)?;
            for e in res {
                let i = x.len();
                let s: i64 = q.iter().enumerate().map(|(j, c)| c * i64::from(x[i - j - 1])).sum();
                x.push(((s >> shift) + i64::from(e)) as i32);
            }
        }
        _ => return Err(format!("reserved FLAC subframe type {t}")),
    }
    if x.len() != n {
        return Err("FLAC subframe has the wrong length".into());
    }
    if wasted > 0 {
        x.iter_mut().for_each(|v| *v <<= wasted);
    }
    Ok(x)
}

pub fn decode(b: &[u8]) -> Result<Flac, String> {
    if !b.starts_with(b"fLaC") {
        return Err("not a FLAC file".into());
    }
    let mut pos = 4;
    let mut info: Option<Flac> = None;
    loop {
        let h = b.get(pos..pos + 4).ok_or("truncated FLAC metadata")?;
        let last = h[0] & 0x80 != 0;
        let len = (usize::from(h[1]) << 16) | (usize::from(h[2]) << 8) | usize::from(h[3]);
        let body = b.get(pos + 4..pos + 4 + len).ok_or("truncated FLAC metadata")?;
        if h[0] & 0x7F == 0 && len >= 34 {
            let mut r = Reader { b: body, pos: 0 };
            r.bits(16)?;
            r.bits(16)?;
            r.bits(24)?;
            r.bits(24)?;
            let rate = r.bits(20)?;
            let ch = r.bits(3)? as u16 + 1;
            let bps = r.bits(5)? + 1;
            let total = (u64::from(r.bits(4)?) << 32) | u64::from(r.bits(32)?);
            let mut md5 = [0u8; 16];
            md5.copy_from_slice(&body[18..34]);
            info = Some(Flac { sample_rate: rate, channels: ch, bits_per_sample: bps, total_samples: total, md5, samples: Vec::new() });
        }
        pos += 4 + len;
        if last {
            break;
        }
    }
    let mut f = info.ok_or("FLAC file has no STREAMINFO")?;
    while pos + 2 <= b.len() {
        if b[pos] != 0xFF || b[pos + 1] & 0xFC != 0xF8 {
            return Err(format!("lost FLAC frame sync at byte {pos}"));
        }
        let start = pos;
        let mut r = Reader { b, pos: pos * 8 + 16 };
        let bs_code = r.bits(4)?;
        let rate_code = r.bits(4)?;
        let assign = r.bits(4)?;
        let ss = r.bits(3)?;
        r.bits(1)?;
        // UTF-8 coded frame/sample number.
        let lead = r.bits(8)?;
        let extra = (lead as u8).leading_ones().saturating_sub(1);
        for _ in 0..extra {
            r.bits(8)?;
        }
        let n = match bs_code {
            1 => 192,
            2..=5 => 576 << (bs_code - 2),
            6 => r.bits(8)? as usize + 1,
            7 => r.bits(16)? as usize + 1,
            8..=15 => 256 << (bs_code - 8),
            _ => return Err("reserved FLAC block size".into()),
        };
        match rate_code {
            12 => {
                r.bits(8)?;
            }
            13 | 14 => {
                r.bits(16)?;
            }
            _ => {}
        }
        let crc_pos = r.pos / 8;
        let crc = r.bits(8)? as u8;
        if crc8(&b[start..crc_pos]) != crc {
            return Err(format!("FLAC header CRC mismatch at byte {start}"));
        }
        let bps = match ss {
            0 => f.bits_per_sample,
            1 => 8,
            2 => 12,
            4 => 16,
            5 => 20,
            6 => 24,
            7 => 32,
            _ => return Err("reserved FLAC sample size".into()),
        };
        let chans = if assign < 8 { assign as usize + 1 } else { 2 };
        let mut ch: Vec<Vec<i32>> = Vec::with_capacity(chans);
        for c in 0..chans {
            let side = matches!((assign, c), (8, 1) | (9, 0) | (10, 1));
            ch.push(decode_subframe(&mut r, n, bps + u32::from(side))?);
        }
        r.align();
        let end = r.pos / 8;
        let fcrc = u16::from_be_bytes([*b.get(end).ok_or("truncated FLAC frame")?, *b.get(end + 1).ok_or("truncated FLAC frame")?]);
        if crc16(&b[start..end]) != fcrc {
            return Err(format!("FLAC frame CRC mismatch at byte {start}"));
        }
        pos = end + 2;
        match assign {
            8 => {
                for i in 0..n {
                    ch[1][i] = ch[0][i] - ch[1][i];
                }
            }
            9 => {
                for i in 0..n {
                    ch[0][i] += ch[1][i];
                }
            }
            10 => {
                for i in 0..n {
                    let side = ch[1][i];
                    let mid = (ch[0][i] << 1) | (side & 1);
                    ch[0][i] = (mid + side) >> 1;
                    ch[1][i] = (mid - side) >> 1;
                }
            }
            _ => {}
        }
        for i in 0..n {
            for c in &ch {
                f.samples.push(c[i]);
            }
        }
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn md5_hex(data: &[u8]) -> String {
        let mut m = Md5::default();
        // Feed in odd pieces to exercise buffering.
        for c in data.chunks(7) {
            m.update(c);
        }
        m.finish().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_matches_rfc_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(md5_hex(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"), "57edf4a22be3c955ac49da2e2107b67a");
    }

    #[test]
    fn utf8_frame_numbers() {
        let enc = |v| {
            let mut o = Vec::new();
            utf8_number(v, &mut o);
            o
        };
        assert_eq!(enc(0x41), vec![0x41]);
        assert_eq!(enc(0x80), vec![0xC2, 0x80]);
        assert_eq!(enc(0x800), vec![0xE0, 0xA0, 0x80]);
        assert_eq!(enc(0x10000), vec![0xF0, 0x90, 0x80, 0x80]);
    }

    fn signal(frames: usize) -> Vec<i16> {
        let mut seed = 99u32;
        (0..frames)
            .flat_map(|i| {
                let t = i as f64 / 48_000.0;
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = ((seed >> 16) as f64 / 65_536.0 - 0.5) * 200.0;
                let l = (t * 440.0 * std::f64::consts::TAU).sin() * 12_000.0 + noise;
                let r = (t * 660.0 * std::f64::consts::TAU).sin() * 9_000.0 + l * 0.3;
                [l.round().clamp(-32768.0, 32767.0) as i16, r.round().clamp(-32768.0, 32767.0) as i16]
            })
            .collect()
    }

    #[test]
    fn lossless_round_trip_and_compression() {
        for frames in [0usize, 1, 100, 4096, 4097, 20_000] {
            let pcm = signal(frames);
            let mut w = FlacWriter::new(Cursor::new(Vec::new()), 48_000, 2).unwrap();
            for chunk in pcm.chunks(1234 * 2) {
                w.push_i16(chunk).unwrap();
            }
            let bytes = w.finish().unwrap().into_inner();
            let f = decode(&bytes).unwrap();
            assert_eq!((f.sample_rate, f.channels, f.bits_per_sample), (48_000, 2, 16));
            assert_eq!(f.total_samples, frames as u64);
            let back: Vec<i16> = f.samples.iter().map(|&v| v as i16).collect();
            assert_eq!(back, pcm, "{frames} frames");
            let raw: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            assert_eq!(f.md5, { let mut m = Md5::default(); m.update(&raw); m.finish() });
            if frames >= 4096 {
                assert!(bytes.len() * 10 < raw.len() * 7, "{} vs {} raw", bytes.len(), raw.len());
            }
        }
    }

    #[test]
    fn handles_silence_extremes_and_mono() {
        let mut pcm = vec![0i16; 5000 * 2];
        pcm.extend((0..5000).flat_map(|i| if i % 2 == 0 { [i16::MAX, i16::MIN] } else { [i16::MIN, i16::MAX] }));
        let mut w = FlacWriter::new(Cursor::new(Vec::new()), 44_100, 2).unwrap();
        w.push_i16(&pcm).unwrap();
        let f = decode(&w.finish().unwrap().into_inner()).unwrap();
        assert_eq!(f.samples.iter().map(|&v| v as i16).collect::<Vec<_>>(), pcm);

        let mono: Vec<i16> = (0..9000).map(|i| ((i * 37) % 2001) as i16 - 1000).collect();
        let mut w = FlacWriter::new(Cursor::new(Vec::new()), 22_050, 1).unwrap();
        w.push_i16(&mono).unwrap();
        let f = decode(&w.finish().unwrap().into_inner()).unwrap();
        assert_eq!(f.channels, 1);
        assert_eq!(f.samples.iter().map(|&v| v as i16).collect::<Vec<_>>(), mono);
    }

    #[test]
    fn float_input_matches_wav_quantisation() {
        let s = [0.0f32, 0.5, -0.5, 1.0, -1.0, 2.0];
        let mut w = FlacWriter::new(Cursor::new(Vec::new()), 48_000, 2).unwrap();
        w.push_f32(&s).unwrap();
        let f = decode(&w.finish().unwrap().into_inner()).unwrap();
        assert_eq!(f.samples, vec![0, 16384, -16384, 32767, -32767, 32767]);
    }

    /// Writes a FLAC, an AIFF and a GIF to `$STUDIO_SAMPLE_DIR` for checking
    /// with other decoders (`afinfo`, `sips`). Run with `--ignored`.
    #[test]
    #[ignore = "writes sample files for external decoders"]
    fn write_samples_for_external_check() {
        let Ok(dir) = std::env::var("STUDIO_SAMPLE_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let pcm = signal(48_000 * 2);
        let mut w = FlacWriter::new(std::fs::File::create(dir.join("sample.flac")).unwrap(), 48_000, 2).unwrap();
        w.push_i16(&pcm).unwrap();
        w.finish().unwrap();
        let f: Vec<f32> = pcm.iter().map(|&v| f32::from(v) / 32767.0).collect();
        let mut a = crate::aiff::AiffWriter::new(std::fs::File::create(dir.join("sample.aiff")).unwrap(), 48_000, 2).unwrap();
        a.push_f32(&f).unwrap();
        a.finish().unwrap();
        let mut g = crate::gif::GifWriter::new(std::fs::File::create(dir.join("sample.gif")).unwrap(), 160, 90, 0).unwrap();
        for i in 0..5u32 {
            let mut img = crate::RgbaImage::new(160, 90);
            for (k, p) in img.pixels.chunks_exact_mut(4).enumerate() {
                let (x, y) = (k as u32 % 160, k as u32 / 160);
                p.copy_from_slice(&[(x * 255 / 160) as u8, (y * 255 / 90) as u8, ((i * 50 + x) % 256) as u8, 255]);
            }
            g.push(&img, 10, &crate::gif::GifOptions::default()).unwrap();
        }
        g.finish().unwrap();
    }
}
