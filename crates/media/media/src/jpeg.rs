// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! JPEG (JFIF / Exif) decoder: baseline and extended sequential Huffman
//! (SOF0/SOF1) and progressive (SOF2) with 8-bit samples, any sampling
//! factors, restart intervals, grayscale / YCbCr / Adobe RGB / CMYK / YCCK,
//! and the Exif orientation tag. Arithmetic coding, lossless and 12-bit
//! JPEG are rejected with an error.

use crate::RgbaImage;

/// Natural (row-major) index of the k-th coefficient in zig-zag order.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30,
    37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const LOOKUP_BITS: u32 = 9;

#[derive(Clone, Default)]
struct Huffman {
    /// (symbol, code length) for every `LOOKUP_BITS`-bit prefix; length 0 = slow path.
    fast: Vec<(u8, u8)>,
    /// Largest code of each length (1..=16), -1 when none.
    maxcode: [i32; 18],
    /// `valptr[l] - mincode[l]`: index into `symbols` of a code of length l.
    delta: [i32; 17],
    symbols: Vec<u8>,
}

impl Huffman {
    fn new(counts: &[u8; 16], symbols: Vec<u8>) -> Result<Self, String> {
        let mut h = Huffman { fast: vec![(0, 0); 1 << LOOKUP_BITS], maxcode: [-1; 18], delta: [0; 17], symbols };
        let mut code = 0i32;
        let mut k = 0usize;
        for l in 1..=16usize {
            let n = counts[l - 1] as usize;
            if n > 0 {
                h.delta[l] = k as i32 - code;
                for _ in 0..n {
                    if l as u32 <= LOOKUP_BITS {
                        let shift = LOOKUP_BITS - l as u32;
                        let base = (code as usize) << shift;
                        for f in 0..(1usize << shift) {
                            if let Some(e) = h.fast.get_mut(base + f) {
                                *e = (*h.symbols.get(k).ok_or("bad Huffman table")?, l as u8);
                            }
                        }
                    }
                    code += 1;
                    k += 1;
                }
                h.maxcode[l] = code - 1;
            }
            if code > (1 << l) {
                return Err("bad Huffman table".into());
            }
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        Ok(h)
    }
}

/// Bit reader over one entropy-coded segment (stuffed zero bytes removed,
/// restart markers stepped over by `restart`).
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0, acc: 0, n: 0 }
    }

    fn fill(&mut self) {
        while self.n <= 56 {
            let mut b = 0u8;
            if self.pos < self.data.len() {
                b = self.data[self.pos];
                if b == 0xFF {
                    let next = self.data.get(self.pos + 1).copied().unwrap_or(0);
                    if next == 0 {
                        self.pos += 2;
                    } else {
                        // A marker: feed zeros without consuming it.
                        b = 0;
                    }
                } else {
                    self.pos += 1;
                }
            }
            self.acc |= (b as u64) << (56 - self.n);
            self.n += 8;
        }
    }

    fn peek(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.fill();
        }
        (self.acc >> (64 - k)) as u32
    }

    fn skip(&mut self, k: u32) {
        self.acc <<= k;
        self.n -= k;
    }

    fn bits(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        let v = self.peek(k);
        self.skip(k);
        v
    }

    fn bit(&mut self) -> bool {
        self.bits(1) == 1
    }

    /// `receive` + `extend` of T.81 F.2.2.1.
    fn signed(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s) as i32;
        if v < (1 << (s - 1)) { v - (1 << s) + 1 } else { v }
    }

    fn decode(&mut self, h: &Huffman) -> Result<u8, String> {
        let p = self.peek(16);
        let (sym, len) = h.fast[(p >> (16 - LOOKUP_BITS)) as usize];
        if len > 0 {
            self.skip(len as u32);
            return Ok(sym);
        }
        for l in (LOOKUP_BITS as usize + 1)..=16 {
            let code = (p >> (16 - l)) as i32;
            if code <= h.maxcode[l] {
                self.skip(l as u32);
                return h.symbols.get((code + h.delta[l]) as usize).copied().ok_or_else(|| "bad Huffman code".to_string());
            }
        }
        Err("bad Huffman code".into())
    }

    /// Drops buffered bits and steps over the next RSTn marker.
    fn restart(&mut self) {
        self.acc = 0;
        self.n = 0;
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF && (0xD0..=0xD7).contains(&self.data[self.pos + 1]) {
                self.pos += 2;
                return;
            }
            self.pos += 1;
        }
    }
}

#[derive(Clone)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Blocks per row / column, padded to whole MCUs.
    bw: usize,
    bh: usize,
    coefs: Vec<i32>,
    pred: i32,
}

struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    comps: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
}

struct Scan {
    comps: Vec<(usize, usize, usize)>, // (component index, dc table, ac table)
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
}

struct State {
    qt: [[u16; 64]; 4],
    dc: [Huffman; 4],
    ac: [Huffman; 4],
    restart: usize,
    adobe: Option<u8>,
    orientation: u16,
    eobrun: u32,
}

fn be16(d: &[u8], i: usize) -> usize {
    ((d[i] as usize) << 8) | d[i + 1] as usize
}

/// Decodes a JPEG file into RGBA (alpha 255), applying Exif orientation.
pub fn decode(data: &[u8]) -> Result<RgbaImage, String> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err("not a JPEG file".into());
    }
    let mut st = State { qt: [[1; 64]; 4], dc: Default::default(), ac: Default::default(), restart: 0, adobe: None, orientation: 1, eobrun: 0 };
    let mut frame: Option<Frame> = None;
    let mut i = 2;
    loop {
        // Next marker (skipping fill bytes).
        while i < data.len() && data[i] != 0xFF {
            i += 1;
        }
        while i < data.len() && data[i] == 0xFF {
            i += 1;
        }
        if i >= data.len() {
            break;
        }
        let marker = data[i];
        i += 1;
        if marker == 0xD9 {
            break;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            continue;
        }
        if i + 2 > data.len() {
            return Err("truncated JPEG".into());
        }
        let len = be16(data, i);
        if len < 2 || i + len > data.len() {
            return Err("truncated JPEG".into());
        }
        let seg = &data[i + 2..i + len];
        i += len;
        match marker {
            0xC0 | 0xC1 | 0xC2 => frame = Some(read_sof(seg, marker == 0xC2)?),
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(if marker >= 0xC9 { "arithmetic-coded JPEG is not supported" } else { "lossless / hierarchical JPEG is not supported" }.into());
            }
            0xC4 => read_dht(seg, &mut st)?,
            0xDB => read_dqt(seg, &mut st)?,
            0xDD => st.restart = if seg.len() >= 2 { be16(seg, 0) } else { 0 },
            0xE1 => {
                if seg.starts_with(b"Exif\0\0") {
                    st.orientation = exif_orientation(&seg[6..]).unwrap_or(1);
                }
            }
            0xEE => {
                if seg.len() >= 12 && seg.starts_with(b"Adobe") {
                    st.adobe = Some(seg[11]);
                }
            }
            0xDA => {
                let f = frame.as_mut().ok_or("scan before frame header")?;
                let scan = read_sos(seg, f)?;
                // The entropy-coded data runs to the next non-RST marker.
                let start = i;
                let mut j = i;
                while j + 1 < data.len() {
                    if data[j] == 0xFF {
                        let n = data[j + 1];
                        if n != 0 && !(0xD0..=0xD7).contains(&n) && n != 0xFF {
                            break;
                        }
                    }
                    j += 1;
                }
                if j + 1 >= data.len() {
                    j = data.len();
                }
                decode_scan(&data[start..j], f, &scan, &mut st)?;
                i = j;
            }
            _ => {}
        }
    }
    let f = frame.ok_or("no image in JPEG")?;
    let img = to_rgba(&f, &st)?;
    Ok(orient(img, st.orientation))
}

pub fn read(path: &std::path::Path) -> Result<RgbaImage, String> {
    decode(&std::fs::read(path).map_err(|e| e.to_string())?)
}

fn read_sof(s: &[u8], progressive: bool) -> Result<Frame, String> {
    if s.len() < 6 {
        return Err("bad frame header".into());
    }
    if s[0] != 8 {
        return Err(format!("{}-bit JPEG is not supported", s[0]));
    }
    let height = be16(s, 1);
    let width = be16(s, 3);
    let n = s[5] as usize;
    if width == 0 || height == 0 {
        return Err("JPEG without a size (DNL) is not supported".into());
    }
    if n == 0 || n > 4 || s.len() < 6 + 3 * n {
        return Err("bad frame header".into());
    }
    let mut comps = Vec::new();
    for c in 0..n {
        let b = &s[6 + 3 * c..9 + 3 * c];
        let (h, v) = ((b[1] >> 4) as usize, (b[1] & 15) as usize);
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || b[2] > 3 {
            return Err("bad sampling factors".into());
        }
        comps.push(Component { id: b[0], h, v, tq: b[2] as usize, bw: 0, bh: 0, coefs: Vec::new(), pred: 0 });
    }
    let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
    let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
    let mcux = width.div_ceil(8 * hmax);
    let mcuy = height.div_ceil(8 * vmax);
    if (mcux * mcuy).saturating_mul(hmax * vmax * n) > 1 << 24 {
        return Err("JPEG is too large".into());
    }
    for c in &mut comps {
        c.bw = mcux * c.h;
        c.bh = mcuy * c.v;
        c.coefs = vec![0; c.bw * c.bh * 64];
    }
    Ok(Frame { width, height, progressive, comps, hmax, vmax, mcux, mcuy })
}

fn read_dht(mut s: &[u8], st: &mut State) -> Result<(), String> {
    while s.len() >= 17 {
        let (class, id) = (s[0] >> 4, (s[0] & 15) as usize);
        if id > 3 || class > 1 {
            return Err("bad Huffman table".into());
        }
        let mut counts = [0u8; 16];
        counts.copy_from_slice(&s[1..17]);
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if s.len() < 17 + total {
            return Err("bad Huffman table".into());
        }
        let h = Huffman::new(&counts, s[17..17 + total].to_vec())?;
        if class == 0 {
            st.dc[id] = h;
        } else {
            st.ac[id] = h;
        }
        s = &s[17 + total..];
    }
    Ok(())
}

fn read_dqt(mut s: &[u8], st: &mut State) -> Result<(), String> {
    while !s.is_empty() {
        let (p, id) = (s[0] >> 4, (s[0] & 15) as usize);
        if id > 3 {
            return Err("bad quantisation table".into());
        }
        let size = if p == 0 { 64 } else { 128 };
        if s.len() < 1 + size {
            return Err("bad quantisation table".into());
        }
        for k in 0..64 {
            st.qt[id][ZIGZAG[k]] = if p == 0 { s[1 + k] as u16 } else { be16(s, 1 + 2 * k) as u16 };
        }
        s = &s[1 + size..];
    }
    Ok(())
}

fn read_sos(s: &[u8], f: &Frame) -> Result<Scan, String> {
    let n = *s.first().ok_or("bad scan header")? as usize;
    if n == 0 || n > 4 || s.len() < 4 + 2 * n {
        return Err("bad scan header".into());
    }
    let mut comps = Vec::new();
    for k in 0..n {
        let id = s[1 + 2 * k];
        let t = s[2 + 2 * k];
        let ci = f.comps.iter().position(|c| c.id == id).ok_or("scan names an unknown component")?;
        comps.push((ci, (t >> 4) as usize & 3, (t & 15) as usize & 3));
    }
    let b = &s[1 + 2 * n..];
    let (ss, se, ah, al) = (b[0] as usize, b[1] as usize, (b[2] >> 4) as u32, (b[2] & 15) as u32);
    if !f.progressive {
        return Ok(Scan { comps, ss: 0, se: 63, ah: 0, al: 0 });
    }
    if se > 63 || ss > se || (ss == 0 && se != 0) || (ss > 0 && n != 1) || al > 13 {
        return Err("bad progressive scan".into());
    }
    Ok(Scan { comps, ss, se, ah, al })
}

fn decode_scan(seg: &[u8], f: &mut Frame, scan: &Scan, st: &mut State) -> Result<(), String> {
    let mut bits = Bits::new(seg);
    for c in &mut f.comps {
        c.pred = 0;
    }
    st.eobrun = 0;
    let single = scan.comps.len() == 1;
    // Units: blocks of the one component (non-interleaved), or MCUs.
    let (ux, uy) = if single {
        let c = &f.comps[scan.comps[0].0];
        ((f.width * c.h).div_ceil(8 * f.hmax), (f.height * c.v).div_ceil(8 * f.vmax))
    } else {
        (f.mcux, f.mcuy)
    };
    let mut todo = st.restart;
    for y in 0..uy {
        for x in 0..ux {
            if st.restart > 0 {
                if todo == 0 {
                    bits.restart();
                    for c in &mut f.comps {
                        c.pred = 0;
                    }
                    st.eobrun = 0;
                    todo = st.restart;
                }
                todo -= 1;
            }
            if single {
                let (ci, dt, at) = scan.comps[0];
                decode_block(&mut bits, f, ci, x, y, dt, at, scan, st)?;
            } else {
                for &(ci, dt, at) in &scan.comps {
                    let (h, v) = (f.comps[ci].h, f.comps[ci].v);
                    for by in 0..v {
                        for bx in 0..h {
                            decode_block(&mut bits, f, ci, x * h + bx, y * v + by, dt, at, scan, st)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn decode_block(bits: &mut Bits, f: &mut Frame, ci: usize, bx: usize, by: usize, dt: usize, at: usize, scan: &Scan, st: &mut State) -> Result<(), String> {
    let c = &mut f.comps[ci];
    if bx >= c.bw || by >= c.bh {
        return Ok(());
    }
    let base = (by * c.bw + bx) * 64;
    let blk = &mut c.coefs[base..base + 64];
    if !f.progressive {
        let t = bits.decode(&st.dc[dt])? as u32;
        c.pred += bits.signed(t);
        blk[0] = c.pred;
        let mut k = 1;
        while k < 64 {
            let rs = bits.decode(&st.ac[at])?;
            let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
            if s == 0 {
                if r == 15 {
                    k += 16;
                    continue;
                }
                break;
            }
            k += r;
            if k > 63 {
                break;
            }
            blk[ZIGZAG[k]] = bits.signed(s);
            k += 1;
        }
        return Ok(());
    }
    if scan.ss == 0 {
        // DC scans.
        if scan.ah == 0 {
            let t = bits.decode(&st.dc[dt])? as u32;
            c.pred += bits.signed(t);
            blk[0] = c.pred * (1 << scan.al);
        } else if bits.bit() {
            blk[0] |= 1 << scan.al;
        }
        return Ok(());
    }
    if scan.ah == 0 {
        // AC first pass.
        if st.eobrun > 0 {
            st.eobrun -= 1;
            return Ok(());
        }
        let mut k = scan.ss;
        while k <= scan.se {
            let rs = bits.decode(&st.ac[at])?;
            let (r, s) = ((rs >> 4) as u32, (rs & 15) as u32);
            if s == 0 {
                if r < 15 {
                    st.eobrun = (1 << r) - 1;
                    if r > 0 {
                        st.eobrun += bits.bits(r);
                    }
                    break;
                }
                k += 16;
                continue;
            }
            k += r as usize;
            if k > 63 {
                break;
            }
            blk[ZIGZAG[k]] = bits.signed(s) * (1 << scan.al);
            k += 1;
        }
        return Ok(());
    }
    // AC refinement (T.81 G.1.2.3, as in libjpeg's decode_mcu_AC_refine).
    let p1 = 1i32 << scan.al;
    let m1 = -1i32 << scan.al;
    let mut k = scan.ss;
    let refine = |bits: &mut Bits, v: &mut i32| {
        if bits.bit() && (*v & p1) == 0 {
            *v += if *v >= 0 { p1 } else { m1 };
        }
    };
    if st.eobrun == 0 {
        while k <= scan.se {
            let rs = bits.decode(&st.ac[at])?;
            let (mut r, s) = ((rs >> 4) as i32, (rs & 15) as u32);
            let mut val = 0;
            if s != 0 {
                val = if bits.bit() { p1 } else { m1 };
            } else if r != 15 {
                st.eobrun = 1 << r;
                if r > 0 {
                    st.eobrun += bits.bits(r as u32);
                }
                break;
            }
            while k <= scan.se {
                let z = ZIGZAG[k];
                if blk[z] != 0 {
                    refine(bits, &mut blk[z]);
                } else {
                    r -= 1;
                    if r < 0 {
                        break;
                    }
                }
                k += 1;
            }
            if val != 0 && k <= scan.se {
                blk[ZIGZAG[k]] = val;
            }
            k += 1;
        }
    }
    if st.eobrun > 0 {
        while k <= scan.se {
            let z = ZIGZAG[k];
            if blk[z] != 0 {
                refine(bits, &mut blk[z]);
            }
            k += 1;
        }
        st.eobrun -= 1;
    }
    Ok(())
}

/// Dequantises and inverse-transforms every block into 8-bit sample planes.
fn planes(f: &Frame, st: &State) -> Vec<(Vec<u8>, usize, usize)> {
    let mut cos = [[0f32; 8]; 8];
    for (x, row) in cos.iter_mut().enumerate() {
        for (u, c) in row.iter_mut().enumerate() {
            let cu = if u == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
            *c = cu * (((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI) / 16.0).cos() / 2.0;
        }
    }
    f.comps
        .iter()
        .map(|c| {
            let (pw, ph) = (c.bw * 8, c.bh * 8);
            let mut out = vec![0u8; pw * ph];
            let q = &st.qt[c.tq];
            let mut tmp = [0f32; 64];
            let mut d = [0f32; 64];
            for by in 0..c.bh {
                for bx in 0..c.bw {
                    let blk = &c.coefs[(by * c.bw + bx) * 64..][..64];
                    for i in 0..64 {
                        d[i] = (blk[i] * q[i] as i32) as f32;
                    }
                    // Rows: tmp[v][x] = sum_u C(x,u) F(v,u).
                    for v in 0..8 {
                        for x in 0..8 {
                            let mut s = 0.0;
                            for u in 0..8 {
                                s += cos[x][u] * d[v * 8 + u];
                            }
                            tmp[v * 8 + x] = s;
                        }
                    }
                    for x in 0..8 {
                        for y in 0..8 {
                            let mut s = 0.0;
                            for v in 0..8 {
                                s += cos[y][v] * tmp[v * 8 + x];
                            }
                            out[(by * 8 + y) * pw + bx * 8 + x] = (s + 128.0).round().clamp(0.0, 255.0) as u8;
                        }
                    }
                }
            }
            (out, pw, ph)
        })
        .collect()
}

fn to_rgba(f: &Frame, st: &State) -> Result<RgbaImage, String> {
    let planes = planes(f, st);
    let (w, h) = (f.width, f.height);
    // Sample component `c` at image pixel (x, y): bilinear on the subsampled grid.
    let sample = |c: usize, x: usize, y: usize| -> f32 {
        let comp = &f.comps[c];
        let (p, pw, ph) = &planes[c];
        if comp.h == f.hmax && comp.v == f.vmax {
            return p[y * pw + x] as f32;
        }
        #[cfg(test)]
        if REPLICATE.with(|r| r.get()) {
            return p[(y * comp.v / f.vmax) * pw + x * comp.h / f.hmax] as f32;
        }
        let fx = ((x as f32 + 0.5) * comp.h as f32 / f.hmax as f32 - 0.5).max(0.0);
        let fy = ((y as f32 + 0.5) * comp.v as f32 / f.vmax as f32 - 0.5).max(0.0);
        let (x0, y0) = (fx as usize, fy as usize);
        let cw = ((w * comp.h).div_ceil(f.hmax)).min(*pw);
        let chh = ((h * comp.v).div_ceil(f.vmax)).min(*ph);
        let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(chh - 1));
        let (x0, y0) = (x0.min(cw - 1), y0.min(chh - 1));
        let (ax, ay) = (fx - fx.floor(), fy - fy.floor());
        let at = |xx: usize, yy: usize| p[yy * pw + xx] as f32;
        let top = at(x0, y0) * (1.0 - ax) + at(x1, y0) * ax;
        let bot = at(x0, y1) * (1.0 - ax) + at(x1, y1) * ax;
        top * (1.0 - ay) + bot * ay
    };
    let n = f.comps.len();
    let ids: Vec<u8> = f.comps.iter().map(|c| c.id).collect();
    // Colour transform: Adobe flag when present, else JFIF YCbCr (unless the
    // components are literally named R, G, B).
    let transform = match st.adobe {
        Some(t) => t,
        None if n == 3 && ids == [b'R', b'G', b'B'] => 0,
        None if n == 3 => 1,
        None => 0,
    };
    let ycc = |y: f32, cb: f32, cr: f32| -> [f32; 3] { [y + 1.402 * (cr - 128.0), y - 0.344_136 * (cb - 128.0) - 0.714_136 * (cr - 128.0), y + 1.772 * (cb - 128.0)] };
    let mut px = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let rgb = match n {
                1 => {
                    let g = sample(0, x, y);
                    [g, g, g]
                }
                3 => {
                    let (a, b, c) = (sample(0, x, y), sample(1, x, y), sample(2, x, y));
                    if transform == 0 { [a, b, c] } else { ycc(a, b, c) }
                }
                4 => {
                    let (a, b, c, k) = (sample(0, x, y), sample(1, x, y), sample(2, x, y), sample(3, x, y));
                    let cmy = if transform == 2 { ycc(a, b, c) } else { [a, b, c] };
                    // Adobe stores inverted CMYK.
                    [cmy[0] * k / 255.0, cmy[1] * k / 255.0, cmy[2] * k / 255.0]
                }
                _ => return Err(format!("{n}-component JPEG is not supported")),
            };
            let o = (y * w + x) * 4;
            for k in 0..3 {
                px[o + k] = rgb[k].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(RgbaImage { width: w as u32, height: h as u32, pixels: px })
}

/// Orientation (tag 0x0112) from an Exif TIFF block.
fn exif_orientation(t: &[u8]) -> Option<u16> {
    let le = match t.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b = t.get(i..i + 2)?;
        Some(if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b = t.get(i..i + 4)?;
        Some(if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    for e in 0..count {
        let p = ifd + 2 + 12 * e;
        if u16_at(p)? == 0x0112 {
            return u16_at(p + 8).filter(|o| (1..=8).contains(o));
        }
    }
    None
}

/// Applies an Exif orientation so the picture displays upright.
fn orient(img: RgbaImage, o: u16) -> RgbaImage {
    if o <= 1 || o > 8 {
        return img;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let swap = o >= 5;
    let (ow, oh) = if swap { (h, w) } else { (w, h) };
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                _ => (y, w - 1 - x),
            };
            let s = (y * w + x) * 4;
            let d = (ny * ow + nx) * 4;
            out[d..d + 4].copy_from_slice(&img.pixels[s..s + 4]);
        }
    }
    RgbaImage { width: ow as u32, height: oh as u32, pixels: out }
}

#[cfg(test)]
thread_local! {
    /// Tests: replicate chroma like libjpeg's plain upsampler instead of interpolating.
    static REPLICATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Option<Vec<u8>> {
        std::fs::read(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/images").join(name)).ok()
    }

    /// Mean absolute RGB difference against a reference decode (`.rgb`, 8-bit RGB).
    fn diff(img: &RgbaImage, rgb: &[u8]) -> f64 {
        let mut s = 0.0;
        for i in 0..(img.width * img.height) as usize {
            for k in 0..3 {
                s += (img.pixels[i * 4 + k] as f64 - rgb[i * 3 + k] as f64).abs();
            }
        }
        s / (img.width * img.height * 3) as f64
    }

    #[test]
    fn decodes_reference_images() {
        let list = fixture("jpeg.txt").map(|t| String::from_utf8(t).unwrap()).unwrap_or_default();
        let mut n = 0;
        for line in list.lines().filter(|l| !l.trim().is_empty()) {
            let v: Vec<&str> = line.split_whitespace().collect();
            let (name, w, h) = (v[0], v[1].parse::<u32>().unwrap(), v[2].parse::<u32>().unwrap());
            // libjpeg interpolates 2x1 / 2x2 chroma and replicates other
            // factors; our default always interpolates, so accept either.
            let rgb = fixture(&format!("{name}.rgb")).unwrap();
            let d = [false, true]
                .into_iter()
                .map(|rep| {
                    REPLICATE.with(|r| r.set(rep));
                    let img = decode(&fixture(&format!("{name}.jpg")).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert_eq!((img.width, img.height), (w, h), "{name}");
                    diff(&img, &rgb)
                })
                .fold(f64::MAX, f64::min);
            REPLICATE.with(|r| r.set(false));
            assert!(d < 1.0, "{name}: mean difference {d:.3}");
            n += 1;
        }
        assert!(n > 0, "no JPEG fixtures");
    }

    #[test]
    #[ignore]
    fn luma_vs_chroma_error() {
        let name = std::env::var("JPEG_NAME").unwrap();
        REPLICATE.with(|r| r.set(std::env::var("JPEG_REPLICATE").is_ok()));
        let img = decode(&fixture(&format!("{name}.jpg")).unwrap()).unwrap();
        let rgb = fixture(&format!("{name}.rgb")).unwrap();
        let (mut ly, mut lc) = (0.0, 0.0);
        let n = (img.width * img.height) as usize;
        for i in 0..n {
            let a = &img.pixels[i * 4..i * 4 + 3];
            let b = &rgb[i * 3..i * 3 + 3];
            let y = |p: &[u8]| 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
            ly += (y(a) - y(b)).abs();
            lc += (a[0] as f64 - y(a) - (b[0] as f64 - y(b))).abs();
        }
        eprintln!("{name}: luma {:.3} chroma(R-Y) {:.3}", ly / n as f64, lc / n as f64);
    }

    #[test]
    fn rejects_non_jpeg() {
        assert!(decode(b"\x89PNG....").is_err());
        assert!(decode(&[0xFF, 0xD8, 0xFF, 0xC0, 0, 0]).is_err());
    }

    #[test]
    fn orientation_six_rotates_clockwise() {
        let img = RgbaImage { width: 2, height: 1, pixels: vec![1, 0, 0, 255, 2, 0, 0, 255] };
        let r = orient(img, 6);
        assert_eq!((r.width, r.height, r.pixels[0], r.pixels[4]), (1, 2, 1, 2));
    }
}
