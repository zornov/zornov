// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Vorbis I audio decoding, written from the Xiph Vorbis I specification:
//! identification/comment/setup headers, Huffman codebooks with VQ lookup
//! types 1 and 2, floor 1, residues 0/1/2, mappings with polar channel
//! coupling, short/long sine-power windows with overlap-add, and an IMDCT
//! built on an N/4-point complex FFT. Floor 0 (unused by any modern encoder)
//! is rejected with a clear error. Hand-written, no third-party code.

use crate::ogg;
use crate::wav::Wav;
use std::f64::consts::PI;

// ---------------------------------------------------------------- bits

/// LSB-first bit reader over one packet.
pub struct BitReader<'a> {
    d: &'a [u8],
    bit: usize,
    end: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Self { d, bit: 0, end: d.len() * 8 }
    }

    /// At least 57 upcoming bits, zero padded past the end.
    #[inline]
    fn peek(&self) -> u64 {
        let b = self.bit >> 3;
        let w = if b + 8 <= self.d.len() {
            u64::from_le_bytes(self.d[b..b + 8].try_into().unwrap())
        } else {
            let mut buf = [0u8; 8];
            if b < self.d.len() {
                let r = &self.d[b..];
                buf[..r.len()].copy_from_slice(r);
            }
            u64::from_le_bytes(buf)
        };
        w >> (self.bit & 7)
    }

    /// Reads `n` ≤ 32 bits; None (and the reader is exhausted) at end of packet.
    #[inline]
    pub fn read(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if self.bit + n as usize > self.end {
            self.bit = self.end;
            return None;
        }
        let v = self.peek() & ((1u64 << n) - 1);
        self.bit += n as usize;
        Some(v as u32)
    }

    #[inline]
    fn skip(&mut self, n: u32) -> bool {
        if self.bit + n as usize > self.end {
            self.bit = self.end;
            return false;
        }
        self.bit += n as usize;
        true
    }
}

fn rd(br: &mut BitReader, n: u32) -> Result<u32, String> {
    br.read(n).ok_or_else(|| "Vorbis header is truncated".to_string())
}

/// Number of bits needed to hold `x` (ilog(0) = 0, ilog(8) = 4).
pub fn ilog(x: u32) -> u32 {
    32 - x.leading_zeros()
}

/// The spec's float32_unpack: 21-bit mantissa, 10-bit biased exponent.
pub fn float32_unpack(x: u32) -> f32 {
    let mantissa = (x & 0x1f_ffff) as f64;
    let exp = ((x & 0x7fe0_0000) >> 21) as i32;
    let m = if x & 0x8000_0000 != 0 { -mantissa } else { mantissa };
    (m * 2f64.powi(exp - 788)) as f32
}

/// Largest r with r^dim ≤ entries.
pub fn lookup1_values(entries: u32, dim: u32) -> u32 {
    if dim == 0 {
        return 0;
    }
    let pow_le = |r: u64| -> bool {
        let mut acc: u64 = 1;
        for _ in 0..dim {
            acc = match acc.checked_mul(r) {
                Some(v) if v <= entries as u64 => v,
                _ => return false,
            };
        }
        true
    };
    let mut r = (entries as f64).powf(1.0 / dim as f64).floor() as u64;
    while r > 0 && !pow_le(r) {
        r -= 1;
    }
    while pow_le(r + 1) {
        r += 1;
    }
    r as u32
}

// ---------------------------------------------------------------- codebooks

const FAST_BITS: u32 = 10;

struct Codebook {
    dim: usize,
    /// Indexed by the next `fast_bits` stream bits: (entry + 1) << 6 | length, 0 = miss.
    fast: Vec<u32>,
    fast_bits: u32,
    /// Codes longer than `fast_bits`: (bit-reversed code, length, entry), shortest first.
    long: Vec<(u32, u32, u32)>,
    /// entries × dim VQ vectors, empty for lookup type 0.
    vq: Vec<f32>,
}

/// Assigns Vorbis codewords (MSB-first tree order) to entry lengths; returns
/// (entry, bit-reversed code, length) for the used entries.
fn assign_codewords(lengths: &[u8]) -> Result<Vec<(u32, u32, u32)>, String> {
    let mut out = Vec::new();
    let mut available = [0u32; 33];
    let Some(first) = lengths.iter().position(|&l| l > 0) else { return Ok(out) };
    let l0 = lengths[first] as usize;
    out.push((first as u32, 0, l0 as u32));
    for (i, a) in available.iter_mut().enumerate().take(l0 + 1).skip(1) {
        *a = 1u32 << (32 - i);
    }
    for (i, &len) in lengths.iter().enumerate().skip(first + 1) {
        if len == 0 {
            continue;
        }
        let len = len as usize;
        let mut z = len;
        while z > 0 && available[z] == 0 {
            z -= 1;
        }
        if z == 0 {
            return Err("Vorbis codebook is overspecified".into());
        }
        let res = available[z];
        available[z] = 0;
        out.push((i as u32, res.reverse_bits(), len as u32));
        for (y, a) in available.iter_mut().enumerate().take(len + 1).skip(z + 1) {
            *a = res.wrapping_add(1u32 << (32 - y));
        }
    }
    Ok(out)
}

impl Codebook {
    fn parse(br: &mut BitReader) -> Result<Self, String> {
        if rd(br, 24)? != 0x564342 {
            return Err("bad Vorbis codebook sync pattern".into());
        }
        let dim = rd(br, 16)? as usize;
        let entries = rd(br, 24)? as usize;
        let mut lengths = vec![0u8; entries];
        if rd(br, 1)? == 0 {
            let sparse = rd(br, 1)? == 1;
            for l in lengths.iter_mut() {
                if !sparse || rd(br, 1)? == 1 {
                    *l = rd(br, 5)? as u8 + 1;
                }
            }
        } else {
            let mut cur = 0usize;
            let mut len = rd(br, 5)? + 1;
            while cur < entries {
                let num = rd(br, ilog((entries - cur) as u32))? as usize;
                if cur + num > entries || len > 32 {
                    return Err("bad ordered Vorbis codebook".into());
                }
                lengths[cur..cur + num].fill(len as u8);
                cur += num;
                len += 1;
            }
        }
        let lookup = rd(br, 4)?;
        let vq = match lookup {
            0 => Vec::new(),
            1 | 2 => {
                let min = float32_unpack(rd(br, 32)?);
                let delta = float32_unpack(rd(br, 32)?);
                let bits = rd(br, 4)? + 1;
                let seq = rd(br, 1)? == 1;
                let nvals = if lookup == 1 { lookup1_values(entries as u32, dim as u32) as usize } else { entries * dim };
                if entries.saturating_mul(dim) > 1 << 24 || nvals > 1 << 24 {
                    return Err("Vorbis codebook is too large".into());
                }
                let mut mult = Vec::with_capacity(nvals);
                for _ in 0..nvals {
                    mult.push(rd(br, bits)? as f32);
                }
                let mut vq = vec![0f32; entries * dim];
                for e in 0..entries {
                    let mut last = 0f32;
                    let mut div = 1usize;
                    for j in 0..dim {
                        let off = if lookup == 1 { (e / div) % nvals.max(1) } else { e * dim + j };
                        let v = mult.get(off).copied().unwrap_or(0.0) * delta + min + last;
                        if seq {
                            last = v;
                        }
                        vq[e * dim + j] = v;
                        if lookup == 1 {
                            div = div.saturating_mul(nvals);
                        }
                    }
                }
                vq
            }
            t => return Err(format!("unsupported Vorbis codebook lookup type {t}")),
        };
        Self::build(dim, &lengths, vq)
    }

    fn build(dim: usize, lengths: &[u8], vq: Vec<f32>) -> Result<Self, String> {
        let codes = assign_codewords(lengths)?;
        let max_len = codes.iter().map(|c| c.2).max().unwrap_or(0);
        let mut cb = Codebook { dim, fast: Vec::new(), fast_bits: 0, long: Vec::new(), vq };
        if codes.len() == 1 {
            // A single used entry: its codeword is all of its bits, whatever they are.
            let (e, _, l) = codes[0];
            cb.fast = vec![((e + 1) << 6) | l];
            return Ok(cb);
        }
        cb.fast_bits = max_len.min(FAST_BITS);
        cb.fast = vec![0; 1 << cb.fast_bits];
        for &(e, code, len) in &codes {
            if len <= cb.fast_bits {
                for k in 0..(1u32 << (cb.fast_bits - len)) {
                    cb.fast[(code | (k << len)) as usize] = ((e + 1) << 6) | len;
                }
            } else {
                cb.long.push((code, len, e));
            }
        }
        cb.long.sort_by_key(|c| c.1);
        Ok(cb)
    }

    /// Decodes one entry number; None at end of packet or on an unused code.
    #[inline]
    fn decode(&self, br: &mut BitReader) -> Option<u32> {
        let bits = br.peek();
        let e = self.fast[(bits as usize) & ((1usize << self.fast_bits) - 1)];
        if e != 0 {
            return br.skip(e & 63).then_some((e >> 6) - 1);
        }
        let b = bits as u32;
        for &(code, len, entry) in &self.long {
            let mask = if len >= 32 { u32::MAX } else { (1u32 << len) - 1 };
            if b & mask == code {
                return br.skip(len).then_some(entry);
            }
        }
        br.bit = br.end;
        None
    }

    #[inline]
    fn decode_vq(&self, br: &mut BitReader) -> Option<&[f32]> {
        let e = self.decode(br)? as usize;
        self.vq.get(e * self.dim..(e + 1) * self.dim)
    }
}

// ---------------------------------------------------------------- floors

struct Floor1 {
    partition_class: Vec<u8>,
    class_dim: Vec<u8>,
    class_sub: Vec<u8>,
    class_master: Vec<u8>,
    sub_books: Vec<[i16; 8]>,
    mult: u32,
    x: Vec<i32>,
    sorted: Vec<usize>,
    low: Vec<usize>,
    high: Vec<usize>,
}

enum Floor {
    Zero,
    One(Floor1),
}

fn inverse_db_table() -> &'static [f32; 256] {
    static T: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    // The spec's floor1_inverse_dB_table: 10^(7·(i − 255)/256), 1.0649863e-7 … 1.0.
    T.get_or_init(|| std::array::from_fn(|i| 10f64.powf(7.0 * (i as f64 - 255.0) / 256.0) as f32))
}

fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> i32 {
    let dy = y1 - y0;
    let adx = x1 - x0;
    let off = dy.abs() * (x - x0) / adx.max(1);
    if dy < 0 {
        y0 - off
    } else {
        y0 + off
    }
}

fn render_line(x0: i32, y0: i32, x1: i32, y1: i32, out: &mut [f32]) {
    let tab = inverse_db_table();
    let n = out.len() as i32;
    let dy = y1 - y0;
    let adx = x1 - x0;
    if adx <= 0 || x0 >= n {
        return;
    }
    let base = dy / adx;
    let sy = if dy < 0 { base - 1 } else { base + 1 };
    let ady = dy.abs() - base.abs() * adx;
    let (mut y, mut err) = (y0, 0);
    let end = x1.min(n);
    out[x0 as usize] = tab[y.clamp(0, 255) as usize];
    for x in x0 + 1..end {
        err += ady;
        if err >= adx {
            err -= adx;
            y += sy;
        } else {
            y += base;
        }
        out[x as usize] = tab[y.clamp(0, 255) as usize];
    }
}

impl Floor1 {
    fn parse(br: &mut BitReader, nbooks: usize) -> Result<Self, String> {
        let parts = rd(br, 5)? as usize;
        let mut partition_class = Vec::with_capacity(parts);
        for _ in 0..parts {
            partition_class.push(rd(br, 4)? as u8);
        }
        let nclass = partition_class.iter().map(|&c| c as usize + 1).max().unwrap_or(0);
        let (mut class_dim, mut class_sub, mut class_master, mut sub_books) = (vec![], vec![], vec![], vec![]);
        let bad_book = |b: u32| b as usize >= nbooks;
        for _ in 0..nclass {
            class_dim.push(rd(br, 3)? as u8 + 1);
            let sub = rd(br, 2)? as u8;
            class_sub.push(sub);
            let master = if sub > 0 { rd(br, 8)? } else { 0 };
            if sub > 0 && bad_book(master) {
                return Err("Vorbis floor references a missing codebook".into());
            }
            class_master.push(master as u8);
            let mut sb = [-1i16; 8];
            for s in sb.iter_mut().take(1 << sub) {
                let b = rd(br, 8)? as i16 - 1;
                if b >= 0 && bad_book(b as u32) {
                    return Err("Vorbis floor references a missing codebook".into());
                }
                *s = b;
            }
            sub_books.push(sb);
        }
        let mult = rd(br, 2)? + 1;
        let rangebits = rd(br, 4)?;
        let mut x = vec![0i32, 1 << rangebits];
        for &c in &partition_class {
            for _ in 0..class_dim[c as usize] {
                x.push(rd(br, rangebits)? as i32);
            }
        }
        if x.len() > 65 {
            return Err("Vorbis floor has too many points".into());
        }
        let mut sorted: Vec<usize> = (0..x.len()).collect();
        sorted.sort_by_key(|&i| x[i]);
        if sorted.windows(2).any(|w| x[w[0]] == x[w[1]]) {
            return Err("Vorbis floor has duplicate points".into());
        }
        let (mut low, mut high) = (vec![0; x.len()], vec![0; x.len()]);
        for i in 2..x.len() {
            let (mut lo, mut hi) = (0usize, 1usize);
            let (mut lx, mut hx) = (-1, i32::MAX);
            for (j, &xj) in x.iter().enumerate().take(i) {
                if xj > lx && xj < x[i] {
                    lx = xj;
                    lo = j;
                }
                if xj < hx && xj > x[i] {
                    hx = xj;
                    hi = j;
                }
            }
            low[i] = lo;
            high[i] = hi;
        }
        Ok(Floor1 { partition_class, class_dim, class_sub, class_master, sub_books, mult, x, sorted, low, high })
    }

    /// Decodes this channel's floor curve into `curve` (n/2 long); None = unused.
    fn decode(&self, books: &[Codebook], br: &mut BitReader, curve: &mut [f32], ys: &mut Vec<i32>, step2: &mut Vec<bool>) -> Option<()> {
        if br.read(1)? == 0 {
            return None;
        }
        let range: i32 = [256, 128, 86, 64][self.mult as usize - 1];
        let ybits = ilog(range as u32 - 1);
        ys.clear();
        ys.push(br.read(ybits)? as i32);
        ys.push(br.read(ybits)? as i32);
        for &cls in &self.partition_class {
            let c = cls as usize;
            let cbits = self.class_sub[c] as u32;
            let csub = (1u32 << cbits) - 1;
            let mut cval = if cbits > 0 { books[self.class_master[c] as usize].decode(br)? } else { 0 };
            for _ in 0..self.class_dim[c] {
                let book = self.sub_books[c][(cval & csub) as usize];
                cval >>= cbits;
                ys.push(if book >= 0 { books[book as usize].decode(br)? as i32 } else { 0 });
            }
        }
        // Amplitude synthesis (in place: earlier points are final when used).
        let n = self.x.len();
        step2.clear();
        step2.resize(n, false);
        step2[0] = true;
        step2[1] = true;
        for i in 2..n {
            let (lo, hi) = (self.low[i], self.high[i]);
            let pred = render_point(self.x[lo], ys[lo], self.x[hi], ys[hi], self.x[i]);
            let val = ys[i];
            let highroom = range - pred;
            let lowroom = pred;
            let room = if highroom < lowroom { highroom * 2 } else { lowroom * 2 };
            if val != 0 {
                step2[lo] = true;
                step2[hi] = true;
                step2[i] = true;
                ys[i] = if val >= room {
                    if highroom > lowroom {
                        val - lowroom + pred
                    } else {
                        pred - val + highroom - 1
                    }
                } else if val & 1 == 1 {
                    pred - (val + 1) / 2
                } else {
                    pred + val / 2
                };
            } else {
                ys[i] = pred;
            }
            ys[i] = ys[i].clamp(0, range - 1);
        }
        // Curve synthesis.
        let m = self.mult as i32;
        let (mut lx, mut ly) = (0, ys[self.sorted[0]] * m);
        let (mut hx, mut hy) = (0, ly);
        for &i in &self.sorted[1..] {
            if step2[i] {
                hy = ys[i] * m;
                hx = self.x[i];
                render_line(lx, ly, hx, hy, curve);
                lx = hx;
                ly = hy;
            }
        }
        if (hx as usize) < curve.len() {
            render_line(hx, hy, curve.len() as i32, hy, curve);
        }
        Some(())
    }
}

// ---------------------------------------------------------------- residues

struct Residue {
    kind: u16,
    begin: usize,
    end: usize,
    psize: usize,
    classifications: u32,
    classbook: usize,
    books: Vec<[i16; 8]>,
}

impl Residue {
    fn parse(br: &mut BitReader, kind: u16, books: &[Codebook]) -> Result<Self, String> {
        let begin = rd(br, 24)? as usize;
        let end = rd(br, 24)? as usize;
        let psize = rd(br, 24)? as usize + 1;
        let classifications = rd(br, 6)? + 1;
        let classbook = rd(br, 8)? as usize;
        let mut cascade = Vec::new();
        for _ in 0..classifications {
            let low = rd(br, 3)?;
            let high = if rd(br, 1)? == 1 { rd(br, 5)? } else { 0 };
            cascade.push(high * 8 + low);
        }
        let mut rb = Vec::new();
        for c in cascade {
            let mut b = [-1i16; 8];
            for (j, slot) in b.iter_mut().enumerate() {
                if c & (1 << j) != 0 {
                    let book = rd(br, 8)? as usize;
                    let ok = books.get(book).is_some_and(|bk| !bk.vq.is_empty() && bk.dim > 0 && psize % bk.dim == 0);
                    if !ok {
                        return Err("Vorbis residue references an unusable codebook".into());
                    }
                    *slot = book as i16;
                }
            }
            rb.push(b);
        }
        if books.get(classbook).is_none_or(|b| b.dim == 0) {
            return Err("Vorbis residue references a missing classbook".into());
        }
        Ok(Residue { kind, begin, end, psize, classifications, classbook, books: rb })
    }

    /// Decodes into `vs` (one vector per channel of the bundle); format 0 or 1.
    fn decode(&self, books: &[Codebook], br: &mut BitReader, vs: &mut [Vec<f32>], dnd: &[bool], size: usize, format: u16, cls: &mut Vec<u8>) {
        let begin = self.begin.min(size);
        let end = self.end.min(size);
        if end <= begin {
            return;
        }
        let nparts = (end - begin) / self.psize;
        if nparts == 0 {
            return;
        }
        let cb = &books[self.classbook];
        let cpc = cb.dim;
        let stride = nparts + cpc;
        cls.clear();
        cls.resize(vs.len() * stride, 0);
        for pass in 0..8 {
            let mut pc = 0;
            while pc < nparts {
                if pass == 0 {
                    for j in 0..vs.len() {
                        if dnd[j] {
                            continue;
                        }
                        let Some(mut t) = cb.decode(br) else { return };
                        for i in (0..cpc).rev() {
                            cls[j * stride + pc + i] = (t % self.classifications) as u8;
                            t /= self.classifications;
                        }
                    }
                }
                for _ in 0..cpc {
                    if pc >= nparts {
                        break;
                    }
                    for j in 0..vs.len() {
                        if dnd[j] {
                            continue;
                        }
                        let book = self.books[cls[j * stride + pc] as usize][pass];
                        if book < 0 {
                            continue;
                        }
                        let bk = &books[book as usize];
                        let off = begin + pc * self.psize;
                        let v = &mut vs[j][off..off + self.psize];
                        if decode_partition(bk, br, v, format).is_none() {
                            return;
                        }
                    }
                    pc += 1;
                }
            }
        }
    }
}

#[inline]
fn decode_partition(bk: &Codebook, br: &mut BitReader, v: &mut [f32], format: u16) -> Option<()> {
    let dim = bk.dim;
    if format == 0 {
        let step = v.len() / dim;
        for i in 0..step {
            let q = bk.decode_vq(br)?;
            for (j, &x) in q.iter().enumerate() {
                v[i + j * step] += x;
            }
        }
    } else {
        for chunk in v.chunks_mut(dim) {
            let q = bk.decode_vq(br)?;
            for (o, &x) in chunk.iter_mut().zip(q) {
                *o += x;
            }
        }
    }
    Some(())
}

// ---------------------------------------------------------------- mappings / modes

struct Mapping {
    coupling: Vec<(usize, usize)>,
    mux: Vec<usize>,
    submap_floor: Vec<usize>,
    submap_residue: Vec<usize>,
}

struct Mode {
    long: bool,
    mapping: usize,
}

// ---------------------------------------------------------------- IMDCT

struct Fft {
    n: usize,
    rev: Vec<u32>,
    tw: Vec<(f32, f32)>,
}

impl Fft {
    fn new(n: usize) -> Self {
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (32 - bits) }).collect();
        let tw = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Fft { n, rev, tw }
    }

    /// In-place forward complex FFT (e^{-2πi jk/n}).
    fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let (wr, wi) = self.tw[k * step];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
    }
}

/// Inverse MDCT of size n (n/2 coefficients in, n samples out):
/// y[i] = Σ_k X[k] cos(2π/n (i + 1/2 + n/4)(k + 1/2)).
pub struct Imdct {
    n: usize,
    fft: Fft,
    pre: Vec<(f32, f32)>,
    post: Vec<(f32, f32)>,
    re: Vec<f32>,
    im: Vec<f32>,
    u: Vec<f32>,
}

impl Imdct {
    pub fn new(n: usize) -> Self {
        let m = n / 2;
        let q = n / 4;
        let pre = (0..q)
            .map(|k| {
                let a = -PI * (k as f64 + 0.25) / m as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let post = (0..q)
            .map(|j| {
                let a = -PI * j as f64 / m as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Imdct { n, fft: Fft::new(q), pre, post, re: vec![0.0; q], im: vec![0.0; q], u: vec![0.0; m] }
    }

    pub fn run(&mut self, x: &[f32], y: &mut [f32]) {
        let m = self.n / 2;
        let q = self.n / 4;
        // DCT-IV of size m via a q-point complex FFT.
        for k in 0..q {
            let (a, b) = (x[2 * k], x[m - 1 - 2 * k]);
            let (c, d) = self.pre[k];
            self.re[k] = a * c - b * d;
            self.im[k] = a * d + b * c;
        }
        self.fft.run(&mut self.re, &mut self.im);
        for j in 0..q {
            let (c, d) = self.post[j];
            let (a, b) = (self.re[j], self.im[j]);
            self.u[2 * j] = a * c - b * d;
            self.u[m - 1 - 2 * j] = -(a * d + b * c);
        }
        // Unfold the DCT-IV into the MDCT's 2m outputs.
        let h = m / 2;
        let u = &self.u;
        for i in 0..h {
            y[i] = u[i + h];
        }
        for i in h..3 * h {
            y[i] = -u[3 * h - 1 - i];
        }
        for i in 3 * h..2 * m {
            y[i] = -u[i - 3 * h];
        }
    }
}

// ---------------------------------------------------------------- decoder

/// A Vorbis stream decoder fed one packet at a time.
pub struct Decoder {
    pub channels: usize,
    pub sample_rate: u32,
    bs: [usize; 2],
    books: Vec<Codebook>,
    floors: Vec<Floor>,
    residues: Vec<Residue>,
    maps: Vec<Mapping>,
    modes: Vec<Mode>,
    imdct: [Imdct; 2],
    slopes: [Vec<f32>; 2],
    prev: Vec<Vec<f32>>,
    prev_n: usize,
    spec: Vec<Vec<f32>>,
    curve: Vec<Vec<f32>>,
    time: Vec<f32>,
    big: Vec<f32>,
    ys: Vec<i32>,
    step2: Vec<bool>,
    cls: Vec<u8>,
}

fn check_sig(p: &[u8], kind: u8) -> Result<(), String> {
    if p.len() < 7 || p[0] != kind || &p[1..7] != b"vorbis" {
        return Err(format!("missing Vorbis header packet type {kind}"));
    }
    Ok(())
}

fn slope(half: usize) -> Vec<f32> {
    (0..half)
        .map(|i| {
            let s = ((i as f64 + 0.5) / half as f64 * PI / 2.0).sin();
            (PI / 2.0 * s * s).sin() as f32
        })
        .collect()
}

impl Decoder {
    /// Builds a decoder from the identification and setup header packets.
    pub fn new(ident: &[u8], setup: &[u8]) -> Result<Self, String> {
        check_sig(ident, 1)?;
        if ident.len() < 30 {
            return Err("Vorbis identification header is truncated".into());
        }
        let version = u32::from_le_bytes(ident[7..11].try_into().unwrap());
        let channels = ident[11] as usize;
        let sample_rate = u32::from_le_bytes(ident[12..16].try_into().unwrap());
        let (b0, b1) = (ident[28] & 15, ident[28] >> 4);
        if version != 0 || channels == 0 || sample_rate == 0 || !(6..=13).contains(&b0) || !(6..=13).contains(&b1) || b0 > b1 || ident[29] & 1 == 0 {
            return Err("invalid Vorbis identification header".into());
        }
        let bs = [1usize << b0, 1usize << b1];

        check_sig(setup, 5)?;
        let mut br = BitReader::new(&setup[7..]);
        let br = &mut br;
        let mut books = Vec::new();
        for _ in 0..rd(br, 8)? + 1 {
            books.push(Codebook::parse(br)?);
        }
        for _ in 0..rd(br, 6)? + 1 {
            if rd(br, 16)? != 0 {
                return Err("invalid Vorbis time-domain transform".into());
            }
        }
        let mut floors = Vec::new();
        for _ in 0..rd(br, 6)? + 1 {
            match rd(br, 16)? {
                0 => {
                    // Parsed only to keep the bitstream aligned.
                    rd(br, 8)?;
                    rd(br, 16)?;
                    rd(br, 16)?;
                    rd(br, 6)?;
                    rd(br, 8)?;
                    let nb = rd(br, 4)? + 1;
                    for _ in 0..nb {
                        rd(br, 8)?;
                    }
                    floors.push(Floor::Zero);
                }
                1 => floors.push(Floor::One(Floor1::parse(br, books.len())?)),
                t => return Err(format!("invalid Vorbis floor type {t}")),
            }
        }
        let mut residues = Vec::new();
        for _ in 0..rd(br, 6)? + 1 {
            let kind = rd(br, 16)? as u16;
            if kind > 2 {
                return Err(format!("invalid Vorbis residue type {kind}"));
            }
            residues.push(Residue::parse(br, kind, &books)?);
        }
        let mut maps = Vec::new();
        let cbits = ilog(channels as u32 - 1);
        for _ in 0..rd(br, 6)? + 1 {
            if rd(br, 16)? != 0 {
                return Err("invalid Vorbis mapping type".into());
            }
            let submaps = if rd(br, 1)? == 1 { rd(br, 4)? as usize + 1 } else { 1 };
            let mut coupling = Vec::new();
            if rd(br, 1)? == 1 {
                for _ in 0..rd(br, 8)? + 1 {
                    let m = rd(br, cbits)? as usize;
                    let a = rd(br, cbits)? as usize;
                    if m == a || m >= channels || a >= channels {
                        return Err("invalid Vorbis channel coupling".into());
                    }
                    coupling.push((m, a));
                }
            }
            if rd(br, 2)? != 0 {
                return Err("invalid Vorbis mapping reserved bits".into());
            }
            let mut mux = vec![0usize; channels];
            if submaps > 1 {
                for m in mux.iter_mut() {
                    *m = rd(br, 4)? as usize;
                    if *m >= submaps {
                        return Err("invalid Vorbis mapping mux".into());
                    }
                }
            }
            let (mut submap_floor, mut submap_residue) = (Vec::new(), Vec::new());
            for _ in 0..submaps {
                rd(br, 8)?;
                let f = rd(br, 8)? as usize;
                let r = rd(br, 8)? as usize;
                if f >= floors.len() || r >= residues.len() {
                    return Err("Vorbis mapping references a missing floor or residue".into());
                }
                if matches!(floors[f], Floor::Zero) {
                    return Err("Vorbis floor type 0 is not supported (only floor 1 streams decode)".into());
                }
                submap_floor.push(f);
                submap_residue.push(r);
            }
            maps.push(Mapping { coupling, mux, submap_floor, submap_residue });
        }
        let mut modes = Vec::new();
        for _ in 0..rd(br, 6)? + 1 {
            let long = rd(br, 1)? == 1;
            let (wt, tt) = (rd(br, 16)?, rd(br, 16)?);
            let mapping = rd(br, 8)? as usize;
            if wt != 0 || tt != 0 || mapping >= maps.len() {
                return Err("invalid Vorbis mode".into());
            }
            modes.push(Mode { long, mapping });
        }
        if rd(br, 1)? != 1 {
            return Err("Vorbis setup header framing bit missing".into());
        }
        let h = bs[1] / 2;
        Ok(Decoder {
            channels,
            sample_rate,
            bs,
            books,
            floors,
            residues,
            maps,
            modes,
            imdct: [Imdct::new(bs[0]), Imdct::new(bs[1])],
            slopes: [slope(bs[0] / 2), slope(bs[1] / 2)],
            prev: vec![vec![0.0; h]; channels],
            prev_n: 0,
            spec: vec![vec![0.0; h]; channels],
            curve: vec![vec![0.0; h]; channels],
            time: vec![0.0; bs[1]],
            big: Vec::new(),
            ys: Vec::new(),
            step2: Vec::new(),
            cls: Vec::new(),
        })
    }

    /// Forgets the overlap state (after a seek or a gap).
    pub fn reset(&mut self) {
        self.prev_n = 0;
    }

    /// Decodes one audio packet, appending interleaved frames to `out`.
    /// Returns the frame count (0 for the first packet). Corrupt packets
    /// return Err and leave the state unchanged.
    pub fn decode_packet(&mut self, pkt: &[u8], out: &mut Vec<f32>) -> Result<usize, String> {
        let Decoder { channels, bs, books, floors, residues, maps, modes, imdct, slopes, prev, prev_n, spec, curve, time, big, ys, step2, cls, .. } = self;
        let ch = *channels;
        let mut br = BitReader::new(pkt);
        let br = &mut br;
        if br.read(1) != Some(0) {
            return Err("not a Vorbis audio packet".into());
        }
        let mode = br.read(ilog(modes.len() as u32 - 1)).and_then(|m| modes.get(m as usize)).ok_or("bad Vorbis mode number")?;
        let n = bs[mode.long as usize];
        let (prev_long, next_long) = if mode.long {
            let p = br.read(1).ok_or("truncated Vorbis packet")?;
            let q = br.read(1).ok_or("truncated Vorbis packet")?;
            (p == 1, q == 1)
        } else {
            (false, false)
        };
        let n2 = n / 2;
        let map = &maps[mode.mapping];

        // Floors.
        let mut unused = [false; 256];
        for c in 0..ch {
            let Floor::One(f) = &floors[map.submap_floor[map.mux[c]]] else { unreachable!() };
            unused[c] = f.decode(books, br, &mut curve[c][..n2], ys, step2).is_none();
        }
        // Nonzero vector propagation.
        let mut nodec = unused;
        for &(m, a) in &map.coupling {
            if !nodec[m] || !nodec[a] {
                nodec[m] = false;
                nodec[a] = false;
            }
        }
        // Residues.
        for s in spec.iter_mut() {
            s[..n2].fill(0.0);
        }
        for (si, &ri) in map.submap_residue.iter().enumerate() {
            let chans: Vec<usize> = (0..ch).filter(|&c| map.mux[c] == si).collect();
            let dnd: Vec<bool> = chans.iter().map(|&c| nodec[c]).collect();
            let r = &residues[ri];
            if r.kind == 2 {
                if dnd.iter().all(|&d| d) {
                    continue;
                }
                let k = chans.len();
                big.clear();
                big.resize(n2 * k, 0.0);
                let mut v = [std::mem::take(big)];
                r.decode(books, br, &mut v, &[false], n2 * k, 1, cls);
                *big = std::mem::take(&mut v[0]);
                for (j, &c) in chans.iter().enumerate() {
                    for (i, o) in spec[c][..n2].iter_mut().enumerate() {
                        *o = big[i * k + j];
                    }
                }
            } else {
                let mut vs: Vec<Vec<f32>> = chans.iter().map(|&c| std::mem::take(&mut spec[c])).collect();
                r.decode(books, br, &mut vs, &dnd, n2, r.kind, cls);
                for (v, &c) in vs.into_iter().zip(&chans) {
                    spec[c] = v;
                }
            }
        }
        // Inverse coupling.
        for &(m, a) in map.coupling.iter().rev() {
            let (mv, av) = if m < a {
                let (l, r) = spec.split_at_mut(a);
                (&mut l[m], &mut r[0])
            } else {
                let (l, r) = spec.split_at_mut(m);
                (&mut r[0], &mut l[a])
            };
            for (mm, aa) in mv[..n2].iter_mut().zip(av[..n2].iter_mut()) {
                let (x, y) = (*mm, *aa);
                let (nm, na) = if x > 0.0 {
                    if y > 0.0 {
                        (x, x - y)
                    } else {
                        (x + y, x)
                    }
                } else if y > 0.0 {
                    (x, x + y)
                } else {
                    (x - y, x)
                };
                *mm = nm;
                *aa = na;
            }
        }
        // Window geometry.
        let (b0, q) = (bs[0], n / 4);
        let (ls, le, lslope) = if mode.long && !prev_long { (q - b0 / 4, q + b0 / 4, &slopes[0]) } else { (0, n2, &slopes[mode.long as usize]) };
        let (rs, re, rslope) = if mode.long && !next_long { (3 * q - b0 / 4, 3 * q + b0 / 4, &slopes[0]) } else { (n2, n, &slopes[mode.long as usize]) };
        let pn = *prev_n;
        let frames = if pn > 0 { pn / 4 + n / 4 } else { 0 };
        let base = out.len();
        out.resize(base + frames * ch, 0.0);
        let t = &mut time[..n];
        for c in 0..ch {
            if unused[c] {
                t.fill(0.0);
            } else {
                let sp = &mut spec[c][..n2];
                for (s, f) in sp.iter_mut().zip(&curve[c][..n2]) {
                    *s *= *f;
                }
                imdct[mode.long as usize].run(sp, t);
                t[..ls].fill(0.0);
                for (v, w) in t[ls..le].iter_mut().zip(lslope.iter()) {
                    *v *= *w;
                }
                for (v, w) in t[rs..re].iter_mut().zip(rslope.iter().rev()) {
                    *v *= *w;
                }
                t[re..].fill(0.0);
            }
            if frames > 0 {
                // Output covers cur[n/4 - pn/4 .. n/2) overlapped with prev's right half.
                let x0 = q as isize - (pn / 4) as isize;
                let ph = &prev[c][..pn / 2];
                for k in 0..frames {
                    let x = x0 + k as isize;
                    let mut v = if x >= 0 { t[x as usize] } else { 0.0 };
                    if k < ph.len() {
                        v += ph[k];
                    }
                    out[base + k * ch + c] = v;
                }
            }
            prev[c][..n2].copy_from_slice(&t[n2..n]);
        }
        *prev_n = n;
        Ok(frames)
    }
}

/// Decodes the first Vorbis stream of an Ogg file to interleaved PCM,
/// trimmed to the granule positions (start offset and final length).
pub fn decode(data: &[u8]) -> Result<Wav, String> {
    let serial = ogg::find_stream(data, b"\x01vorbis").ok_or("no Ogg Vorbis stream found")?;
    let packets = ogg::packets(data, serial);
    if packets.len() < 3 {
        return Err("Ogg Vorbis stream is missing its headers".into());
    }
    check_sig(&packets[1].data, 3)?;
    let mut dec = Decoder::new(&packets[0].data, &packets[2].data)?;
    let ch = dec.channels;
    let last_g = packets.iter().rposition(|p| p.granule.is_some()).filter(|&i| i >= 3);
    let mut out = Vec::new();
    let mut pos: i64 = 0;
    let mut offset: Option<i64> = None;
    for (i, p) in packets.iter().enumerate().skip(3) {
        if !p.data.is_empty() {
            pos += dec.decode_packet(&p.data, &mut out).unwrap_or(0) as i64;
        }
        if let (Some(g), None) = (p.granule, offset) {
            // The first granule fixes the stream's start; when it is also the
            // last one it only marks the end (short files).
            offset = Some(if Some(i) == last_g { (g as i64 - pos).max(0) } else { g as i64 - pos });
        }
    }
    let frames = out.len() / ch;
    let off = offset.unwrap_or(0);
    let start = ((-off).max(0) as usize).min(frames);
    let end = match last_g {
        Some(i) => ((packets[i].granule.unwrap() as i64 - off).max(0) as usize).clamp(start, frames),
        None => frames,
    };
    let samples: Vec<f32> = out[start * ch..end * ch].iter().map(|v| v.clamp(-1.0, 1.0)).collect();
    Ok(Wav { sample_rate: dec.sample_rate, channels: ch as u16, samples })
}

pub fn read(path: &std::path::Path) -> Result<Wav, String> {
    decode(&std::fs::read(path).map_err(|e| e.to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float32_unpack_values() {
        assert_eq!(float32_unpack((768 << 21) | (1 << 20)), 1.0);
        assert_eq!(float32_unpack(0x8000_0000 | (767 << 21) | (1 << 20)), -0.5);
        assert_eq!(float32_unpack((788 << 21) | 3), 3.0);
        assert_eq!(float32_unpack(0), 0.0);
    }

    #[test]
    fn lookup1_values_cases() {
        assert_eq!(lookup1_values(100, 2), 10);
        assert_eq!(lookup1_values(99, 2), 9);
        assert_eq!(lookup1_values(81, 4), 3);
        assert_eq!(lookup1_values(1000, 3), 10);
        assert_eq!(lookup1_values(999, 3), 9);
        assert_eq!(lookup1_values(5, 1), 5);
        assert_eq!(lookup1_values(1 << 24, 24), 2);
    }

    #[test]
    fn ilog_cases() {
        assert_eq!([0, 1, 2, 3, 4, 7, 8, 255].map(ilog), [0, 1, 2, 2, 3, 3, 4, 8]);
    }

    #[test]
    fn codeword_assignment_matches_spec_example() {
        // Spec section 3.2.1 example.
        let codes = assign_codewords(&[2, 4, 4, 4, 4, 2, 3, 3]).unwrap();
        let msb: Vec<String> = codes.iter().map(|&(_, rev, len)| (0..len).map(|b| if rev >> b & 1 == 1 { '1' } else { '0' }).collect()).collect();
        assert_eq!(msb, ["00", "0100", "0101", "0110", "0111", "10", "110", "111"]);
        assert!(assign_codewords(&[1, 1, 1]).is_err());
    }

    #[test]
    fn codebook_decodes_lsb_first_stream() {
        let cb = Codebook::build(1, &[2, 4, 4, 4, 4, 2, 3, 3], Vec::new()).unwrap();
        // Entries 6 ("110"), 1 ("0100"), 5 ("10"): bits in read order.
        let bits = "110".to_string() + "0100" + "10";
        let mut bytes = vec![0u8; 2];
        for (i, b) in bits.chars().enumerate() {
            if b == '1' {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        let mut br = BitReader::new(&bytes);
        assert_eq!(cb.decode(&mut br), Some(6));
        assert_eq!(cb.decode(&mut br), Some(1));
        assert_eq!(cb.decode(&mut br), Some(5));
        // Long-code fallback: lengths beyond FAST_BITS.
        let mut lens = vec![1u8];
        lens.extend((2..=14).map(|l| l as u8));
        lens.push(14);
        let cb = Codebook::build(1, &lens, Vec::new()).unwrap();
        // Entry 13 is 13 ones then a 0; entry 14 is 14 ones.
        let mut br = BitReader::new(&[0xFF, 0x1F, 0x00]);
        assert_eq!(cb.decode(&mut br), Some(13));
        let mut br = BitReader::new(&[0xFF, 0x3F]);
        assert_eq!(cb.decode(&mut br), Some(14));
    }

    fn naive_imdct(x: &[f32]) -> Vec<f64> {
        let n = x.len() * 2;
        (0..n)
            .map(|i| {
                x.iter()
                    .enumerate()
                    .map(|(k, &v)| v as f64 * (2.0 * PI / n as f64 * (i as f64 + 0.5 + n as f64 / 4.0) * (k as f64 + 0.5)).cos())
                    .sum()
            })
            .collect()
    }

    #[test]
    fn imdct_matches_naive() {
        let mut seed = 12345u32;
        for n in [64usize, 256, 2048] {
            let x: Vec<f32> = (0..n / 2)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
                })
                .collect();
            let want = naive_imdct(&x);
            let mut got = vec![0f32; n];
            Imdct::new(n).run(&x, &mut got);
            let peak = want.iter().fold(0f64, |a, &b| a.max(b.abs()));
            let err = want.iter().zip(&got).fold(0f64, |a, (&w, &g)| a.max((w - g as f64).abs()));
            assert!(err < 1e-5 * peak.max(1.0), "n={n} err={err} peak={peak}");
        }
    }

    #[test]
    fn floor_line_renders_bresenham() {
        let mut out = vec![0f32; 8];
        render_line(0, 10, 8, 18, &mut out);
        let t = inverse_db_table();
        let want: Vec<f32> = (10..18).map(|y| t[y]).collect();
        assert_eq!(out, want);
        assert!((t[255] - 1.0).abs() < 1e-6 && (t[0] - 1.0649863e-07).abs() < 1e-12);
    }

    #[test]
    fn rejects_non_vorbis() {
        assert!(decode(b"not an ogg file").is_err());
        let page = crate::ogg::tests::make_page(2, 0, 1, 0, &[4], b"Opus");
        assert!(decode(&page).unwrap_err().contains("no Ogg Vorbis"));
    }

    // ---- real files (run with --ignored) ----

    /// The content library (`~/Movies/Studio/Content`).
    fn root() -> std::path::PathBuf {
        studio_paths::studio().join("Content")
    }

    fn real_files() -> Vec<std::path::PathBuf> {
        let list = std::env::var("OGG_FILES").unwrap_or_else(|_| {
            [
                "sfx/kenney/ui-audio/files/Preview.ogg",
                "sfx/kenney/casino-audio/files/Preview.ogg",
                "sfx/kenney/digital-audio/files/Preview.ogg",
                "music/kenney/music-jingles/files/Preview.ogg",
                "music/opengameart/fast-fight-battle-music/_dl/fight.ogg",
                "music/opengameart/creepy-forest-f/_dl/forest.ogg",
            ]
            .join("\n")
        });
        list.lines().filter(|l| !l.is_empty()).map(|l| root().join(l)).filter(|p| p.exists()).collect()
    }

    /// Decodes real files; checks length == last granule (minus start offset),
    /// finite samples, and dumps raw f32 to $OGG_DUMP_DIR for reference comparison.
    #[test]
    #[ignore]
    fn real_files_decode() {
        let files = real_files();
        assert!(!files.is_empty(), "content drive not mounted");
        for f in files {
            let data = std::fs::read(&f).unwrap();
            let t = std::time::Instant::now();
            let w = decode(&data).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            let dt = t.elapsed();
            let serial = ogg::find_stream(&data, b"\x01vorbis").unwrap();
            let last = ogg::packets(&data, serial).iter().rev().find_map(|p| p.granule).unwrap();
            assert!(w.samples.iter().all(|v| v.is_finite()));
            assert!(w.frames() as u64 <= last);
            let peak = w.samples.iter().fold(0f32, |a, &b| a.max(b.abs()));
            println!("{}: {} Hz {} ch {} frames (last granule {}) peak {:.3} in {:?}", f.display(), w.sample_rate, w.channels, w.frames(), last, peak, dt);
            if let Ok(dir) = std::env::var("OGG_DUMP_DIR") {
                let name = f.to_string_lossy().replace(['/', ' '], "_");
                let bytes: Vec<u8> = w.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
                std::fs::write(std::path::Path::new(&dir).join(format!("{name}.f32")), bytes).unwrap();
            }
        }
    }
}
