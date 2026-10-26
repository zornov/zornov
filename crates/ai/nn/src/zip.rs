// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hand-written archive readers: zip (stored + deflate, zip64), raw deflate
//! ([`inflate`], RFC 1951), gzip ([`gunzip`], RFC 1952), zlib
//! ([`inflate_zlib`], RFC 1950), CRC-32 and ustar/GNU/pax `tar` listing.
//!
//! Used for MediaPipe `.task` bundles (zip of `.tflite` files) and
//! `.tar.gz` model archives (DeepFilterNet).

use crate::error::{Error, Result};

// ------------------------------------------------------------------ CRC-32

fn crc_table() -> &'static [u32; 256] {
    static T: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    })
}

/// Continues a CRC-32 (IEEE, as used by zip/gzip/png) over `data`.
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let t = crc_table();
    let mut c = !crc;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// CRC-32 (IEEE) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

// ------------------------------------------------------------------ inflate

struct Bits<'a> {
    src: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
    /// Zero bits fed past the end of the input (an error if consumed).
    over: u32,
}

impl<'a> Bits<'a> {
    fn new(src: &'a [u8]) -> Self {
        Bits { src, pos: 0, buf: 0, cnt: 0, over: 0 }
    }
    #[inline]
    fn refill(&mut self) {
        while self.cnt <= 56 {
            let b = if self.pos < self.src.len() {
                let b = self.src[self.pos];
                self.pos += 1;
                b
            } else {
                self.over += 8;
                0
            };
            self.buf |= (b as u64) << self.cnt;
            self.cnt += 8;
        }
    }
    #[inline]
    fn peek(&mut self, n: u32) -> u32 {
        if self.cnt < n {
            self.refill();
        }
        (self.buf & ((1u64 << n) - 1)) as u32
    }
    #[inline]
    fn consume(&mut self, n: u32) -> Result<()> {
        self.buf >>= n;
        self.cnt -= n;
        if self.over > self.cnt {
            return Err(Error("inflate: unexpected end of data".into()));
        }
        Ok(())
    }
    #[inline]
    fn bits(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.peek(n);
        self.consume(n)?;
        Ok(v)
    }
    /// Drops bits up to the next byte boundary.
    fn align(&mut self) -> Result<()> {
        let r = self.cnt % 8;
        self.consume(r)
    }
    /// Input bytes consumed so far (after [`Bits::align`]).
    fn byte_pos(&self) -> usize {
        let unread = (self.cnt.saturating_sub(self.over) / 8) as usize;
        self.pos - unread
    }
}

/// A canonical Huffman decoding table indexed by `max_len` bit-reversed bits.
struct Huff {
    /// `(symbol << 4) | length`, 0 = invalid code.
    table: Vec<u16>,
    max: u32,
}

impl Huff {
    fn new(lens: &[u8]) -> Result<Huff> {
        let max = lens.iter().copied().max().unwrap_or(0) as u32;
        if max == 0 {
            // no codes at all (a distance tree for literal-only data)
            return Ok(Huff { table: vec![0; 2], max: 1 });
        }
        let mut count = [0u32; 16];
        for &l in lens {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut left: i64 = 1;
        for &c in &count[1..] {
            left = left * 2 - c as i64;
            if left < 0 {
                return Err(Error("inflate: over-subscribed Huffman code".into()));
            }
        }
        let mut next = [0u32; 16];
        let mut code = 0u32;
        for l in 1..16 {
            code = (code + count[l - 1]) << 1;
            next[l] = code;
        }
        let size = 1usize << max;
        let mut table = vec![0u16; size];
        for (sym, &l) in lens.iter().enumerate() {
            if l == 0 {
                continue;
            }
            let l = l as u32;
            let c = next[l as usize];
            next[l as usize] += 1;
            let mut rev = 0u32;
            for i in 0..l {
                rev |= ((c >> i) & 1) << (l - 1 - i);
            }
            let entry = ((sym as u16) << 4) | l as u16;
            let mut k = rev as usize;
            while k < size {
                table[k] = entry;
                k += 1 << l;
            }
        }
        Ok(Huff { table, max })
    }
    #[inline]
    fn decode(&self, b: &mut Bits) -> Result<u16> {
        let e = self.table[b.peek(self.max) as usize];
        if e == 0 {
            return Err(Error("inflate: invalid Huffman code".into()));
        }
        b.consume((e & 15) as u32)?;
        Ok(e >> 4)
    }
}

const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

fn fixed_tables() -> Result<(Huff, Huff)> {
    let mut l = [0u8; 288];
    l[..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..].fill(8);
    Ok((Huff::new(&l)?, Huff::new(&[5u8; 30])?))
}

fn dynamic_tables(b: &mut Bits) -> Result<(Huff, Huff)> {
    let hlit = b.bits(5)? as usize + 257;
    let hdist = b.bits(5)? as usize + 1;
    let hclen = b.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(Error("inflate: bad dynamic block header".into()));
    }
    let mut cl = [0u8; 19];
    for &i in CL_ORDER.iter().take(hclen) {
        cl[i] = b.bits(3)? as u8;
    }
    let clh = Huff::new(&cl)?;
    let mut lens = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lens.len() {
        let sym = clh.decode(b)?;
        let (val, rep) = match sym {
            0..=15 => (sym as u8, 1),
            16 => {
                if i == 0 {
                    return Err(Error("inflate: repeat with no previous length".into()));
                }
                (lens[i - 1], 3 + b.bits(2)? as usize)
            }
            17 => (0, 3 + b.bits(3)? as usize),
            _ => (0, 11 + b.bits(7)? as usize),
        };
        if i + rep > lens.len() {
            return Err(Error("inflate: code lengths overflow".into()));
        }
        lens[i..i + rep].fill(val);
        i += rep;
    }
    if lens[256] == 0 {
        return Err(Error("inflate: no end-of-block code".into()));
    }
    Ok((Huff::new(&lens[..hlit])?, Huff::new(&lens[hlit..])?))
}

/// Inflates a raw deflate stream; returns the output and the bytes consumed.
fn inflate_raw(src: &[u8], size_hint: usize) -> Result<(Vec<u8>, usize)> {
    let mut out: Vec<u8> = Vec::with_capacity(size_hint);
    let mut b = Bits::new(src);
    loop {
        let last = b.bits(1)? == 1;
        match b.bits(2)? {
            0 => {
                b.align()?;
                let len = b.bits(16)? as usize;
                let nlen = b.bits(16)? as usize;
                if len != (!nlen & 0xFFFF) {
                    return Err(Error("inflate: stored block length check failed".into()));
                }
                // drain whole bytes still buffered, then copy directly
                let mut n = len;
                while n > 0 && b.cnt >= 8 && b.over < b.cnt {
                    out.push(b.bits(8)? as u8);
                    n -= 1;
                }
                if n > 0 {
                    let p = b.byte_pos();
                    if p + n > src.len() {
                        return Err(Error("inflate: stored block truncated".into()));
                    }
                    out.extend_from_slice(&src[p..p + n]);
                    b = Bits { src, pos: p + n, buf: 0, cnt: 0, over: 0 };
                }
            }
            t @ (1 | 2) => {
                let (lit, dist) = if t == 1 { fixed_tables()? } else { dynamic_tables(&mut b)? };
                loop {
                    let sym = lit.decode(&mut b)? as usize;
                    if sym < 256 {
                        out.push(sym as u8);
                    } else if sym == 256 {
                        break;
                    } else {
                        let li = sym - 257;
                        if li >= 29 {
                            return Err(Error("inflate: bad length symbol".into()));
                        }
                        let len = LEN_BASE[li] as usize + b.bits(LEN_EXTRA[li] as u32)? as usize;
                        let di = dist.decode(&mut b)? as usize;
                        if di >= 30 {
                            return Err(Error("inflate: bad distance symbol".into()));
                        }
                        let d = DIST_BASE[di] as usize + b.bits(DIST_EXTRA[di] as u32)? as usize;
                        if d > out.len() {
                            return Err(Error("inflate: distance too far back".into()));
                        }
                        let start = out.len() - d;
                        if d >= len {
                            out.extend_from_within(start..start + len);
                        } else {
                            for k in 0..len {
                                let v = out[start + k];
                                out.push(v);
                            }
                        }
                    }
                }
            }
            _ => return Err(Error("inflate: reserved block type".into())),
        }
        if last {
            break;
        }
    }
    b.align()?;
    Ok((out, b.byte_pos()))
}

/// Inflates a raw deflate stream (RFC 1951). gzip-wrapped input (magic
/// `1f 8b`, never a valid raw stream) is detected and unwrapped.
pub fn inflate(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.len() >= 3 && raw[0] == 0x1f && raw[1] == 0x8b {
        return gunzip(raw);
    }
    Ok(inflate_raw(raw, raw.len() * 3)?.0)
}

/// Decompresses a zlib stream (RFC 1950), checking the Adler-32 trailer.
pub fn inflate_zlib(data: &[u8]) -> Result<Vec<u8>> {
    crate::ensure!(data.len() >= 6, "zlib: too short");
    let (cmf, flg) = (data[0], data[1]);
    crate::ensure!(cmf & 15 == 8 && (cmf as u16 * 256 + flg as u16) % 31 == 0, "zlib: bad header");
    crate::ensure!(flg & 0x20 == 0, "zlib: preset dictionary not supported");
    let (out, used) = inflate_raw(&data[2..], data.len() * 3)?;
    let t = 2 + used;
    crate::ensure!(t + 4 <= data.len(), "zlib: missing Adler-32");
    let want = u32::from_be_bytes([data[t], data[t + 1], data[t + 2], data[t + 3]]);
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in out.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    crate::ensure!((b << 16 | a) == want, "zlib: Adler-32 mismatch");
    Ok(out)
}

/// Decompresses gzip data (RFC 1952), all members, checking CRC-32 and size.
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut p = 0;
    loop {
        let d = &data[p..];
        crate::ensure!(d.len() >= 18 && d[0] == 0x1f && d[1] == 0x8b, "gzip: bad magic");
        crate::ensure!(d[2] == 8, "gzip: unknown compression method {}", d[2]);
        let flg = d[3];
        let mut h = 10;
        if flg & 4 != 0 {
            crate::ensure!(h + 2 <= d.len(), "gzip: truncated header");
            h += 2 + u16::from_le_bytes([d[h], d[h + 1]]) as usize;
        }
        for bit in [8u8, 16] {
            if flg & bit != 0 {
                while h < d.len() && d[h] != 0 {
                    h += 1;
                }
                h += 1;
            }
        }
        if flg & 2 != 0 {
            h += 2;
        }
        crate::ensure!(h <= d.len(), "gzip: truncated header");
        let (m, used) = inflate_raw(&d[h..], d.len() * 3)?;
        let t = h + used;
        crate::ensure!(t + 8 <= d.len(), "gzip: missing trailer");
        let crc = u32::from_le_bytes([d[t], d[t + 1], d[t + 2], d[t + 3]]);
        let isize = u32::from_le_bytes([d[t + 4], d[t + 5], d[t + 6], d[t + 7]]);
        crate::ensure!(crc32(&m) == crc, "gzip: CRC-32 mismatch");
        crate::ensure!(m.len() as u32 == isize, "gzip: size mismatch");
        out.extend_from_slice(&m);
        p += t + 8;
        // another member? (skip trailing zero padding)
        if p + 2 > data.len() || data[p] != 0x1f || data[p + 1] != 0x8b {
            break;
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ zip

fn u16le(b: &[u8], p: usize) -> Result<u16> {
    b.get(p..p + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| Error("zip: truncated".into()))
}
fn u32le(b: &[u8], p: usize) -> Result<u32> {
    b.get(p..p + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| Error("zip: truncated".into()))
}
fn u64le(b: &[u8], p: usize) -> Result<u64> {
    b.get(p..p + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| Error("zip: truncated".into()))
}

/// One central-directory entry.
#[derive(Clone, Debug)]
pub struct ZipEntry {
    pub name: String,
    /// 0 = stored, 8 = deflate.
    pub method: u16,
    pub crc: u32,
    pub compressed_size: u64,
    pub size: u64,
    pub local_offset: u64,
}

impl ZipEntry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

/// An in-memory zip archive.
pub struct ZipArchive {
    data: Vec<u8>,
    entries: Vec<ZipEntry>,
}

impl ZipArchive {
    /// Parses the central directory (zip64 supported).
    pub fn open(data: Vec<u8>) -> Result<ZipArchive> {
        let b = &data;
        crate::ensure!(b.len() >= 22, "zip: too small");
        let lo = b.len().saturating_sub(22 + 65535);
        let eocd = (lo..=b.len() - 22).rev().find(|&p| b[p..p + 4] == [0x50, 0x4b, 0x05, 0x06]).ok_or_else(|| Error("zip: end of central directory not found".into()))?;
        let mut count = u16le(b, eocd + 10)? as u64;
        let mut cd_off = u32le(b, eocd + 16)? as u64;
        if (count == 0xFFFF || cd_off == 0xFFFF_FFFF) && eocd >= 20 && u32le(b, eocd - 20)? == 0x0706_4b50 {
            let z = u64le(b, eocd - 20 + 8)? as usize;
            crate::ensure!(u32le(b, z)? == 0x0606_4b50, "zip: bad zip64 end record");
            count = u64le(b, z + 32)?;
            cd_off = u64le(b, z + 48)?;
        }
        let mut entries = Vec::with_capacity(count.min(1 << 16) as usize);
        let mut p = cd_off as usize;
        for _ in 0..count {
            crate::ensure!(u32le(b, p)? == 0x0201_4b50, "zip: bad central directory entry");
            let method = u16le(b, p + 10)?;
            let crc = u32le(b, p + 16)?;
            let mut csize = u32le(b, p + 20)? as u64;
            let mut size = u32le(b, p + 24)? as u64;
            let nlen = u16le(b, p + 28)? as usize;
            let xlen = u16le(b, p + 30)? as usize;
            let clen = u16le(b, p + 32)? as usize;
            let mut off = u32le(b, p + 42)? as u64;
            let name_b = b.get(p + 46..p + 46 + nlen).ok_or_else(|| Error("zip: truncated name".into()))?;
            let name = String::from_utf8_lossy(name_b).into_owned();
            // zip64 extra field
            let mut x = p + 46 + nlen;
            let xend = x + xlen;
            while x + 4 <= xend {
                let id = u16le(b, x)?;
                let sz = u16le(b, x + 2)? as usize;
                if id == 1 {
                    let mut q = x + 4;
                    if size == 0xFFFF_FFFF {
                        size = u64le(b, q)?;
                        q += 8;
                    }
                    if csize == 0xFFFF_FFFF {
                        csize = u64le(b, q)?;
                        q += 8;
                    }
                    if off == 0xFFFF_FFFF {
                        off = u64le(b, q)?;
                    }
                }
                x += 4 + sz;
            }
            entries.push(ZipEntry { name, method, crc, compressed_size: csize, size, local_offset: off });
            p += 46 + nlen + xlen + clen;
        }
        Ok(ZipArchive { data, entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }
    /// Entry names in archive order.
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }
    pub fn entry(&self, name: &str) -> Option<&ZipEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Reads and decompresses a member, verifying its CRC-32.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let e = self.entry(name).ok_or_else(|| Error(format!("zip: no member '{name}'")))?;
        self.read_entry(e)
    }

    pub fn read_entry(&self, e: &ZipEntry) -> Result<Vec<u8>> {
        let b = &self.data;
        let p = e.local_offset as usize;
        crate::ensure!(u32le(b, p)? == 0x0403_4b50, "zip: bad local header for '{}'", e.name);
        let start = p + 30 + u16le(b, p + 26)? as usize + u16le(b, p + 28)? as usize;
        let end = start.checked_add(e.compressed_size as usize).filter(|&x| x <= b.len()).ok_or_else(|| Error(format!("zip: '{}' truncated", e.name)))?;
        let raw = &b[start..end];
        let out = match e.method {
            0 => raw.to_vec(),
            8 => inflate_raw(raw, e.size as usize)?.0,
            m => return Err(Error(format!("zip: '{}' uses unsupported method {m}", e.name))),
        };
        crate::ensure!(out.len() as u64 == e.size, "zip: '{}' size mismatch ({} != {})", e.name, out.len(), e.size);
        crate::ensure!(crc32(&out) == e.crc, "zip: '{}' CRC-32 mismatch", e.name);
        Ok(out)
    }
}

// ------------------------------------------------------------------ tar

fn tar_num(f: &[u8]) -> u64 {
    if f.first().is_some_and(|&c| c & 0x80 != 0) {
        // base-256
        let mut v = (f[0] & 0x7F) as u64;
        for &c in &f[1..] {
            v = (v << 8) | c as u64;
        }
        return v;
    }
    let s = String::from_utf8_lossy(f);
    u64::from_str_radix(s.trim_matches(|c: char| c == '\0' || c == ' '), 8).unwrap_or(0)
}

fn cstr(f: &[u8]) -> String {
    let n = f.iter().position(|&c| c == 0).unwrap_or(f.len());
    String::from_utf8_lossy(&f[..n]).into_owned()
}

/// Regular-file members `(path, contents)` of a ustar / GNU / pax tar
/// archive. gzip-compressed input is decompressed first. Stops at the end
/// marker or at the first malformed header.
pub fn untar(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let owned;
    let b: &[u8] = if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        match gunzip(bytes) {
            Ok(v) => {
                owned = v;
                &owned
            }
            Err(_) => return Vec::new(),
        }
    } else {
        bytes
    };
    let mut out = Vec::new();
    let mut p = 0;
    let mut long_name: Option<String> = None;
    while p + 512 <= b.len() {
        let h = &b[p..p + 512];
        if h.iter().all(|&c| c == 0) {
            break;
        }
        // header checksum (sum of header with the checksum field as spaces)
        let want = tar_num(&h[148..156]);
        let sum: u64 = h.iter().enumerate().map(|(i, &c)| if (148..156).contains(&i) { 32 } else { c as u64 }).sum();
        if want != sum {
            break;
        }
        let size = tar_num(&h[124..136]) as usize;
        let kind = h[156];
        let data_start = p + 512;
        let data_end = data_start.saturating_add(size).min(b.len());
        let data = &b[data_start..data_end];
        let mut name = cstr(&h[0..100]);
        if &h[257..262] == b"ustar" {
            let prefix = cstr(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
        }
        match kind {
            b'L' => long_name = Some(cstr(data)),
            b'x' => {
                // pax extended header: "len key=value\n" records
                let text = String::from_utf8_lossy(data);
                for rec in text.split('\n') {
                    if let Some((_, kv)) = rec.split_once(' ')
                        && let Some(path) = kv.strip_prefix("path=")
                    {
                        long_name = Some(path.to_string());
                    }
                }
            }
            b'0' | 0 | b'7' => {
                let n = long_name.take().unwrap_or(name);
                out.push((n, data.to_vec()));
            }
            _ => {
                long_name = None;
            }
        }
        p = data_start + size.div_ceil(512) * 512;
    }
    out
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Test-only bit writer + fixed-Huffman deflate encoder (greedy LZ77).
    struct BitW {
        out: Vec<u8>,
        acc: u64,
        n: u32,
    }
    impl BitW {
        fn put(&mut self, v: u32, n: u32) {
            self.acc |= (v as u64) << self.n;
            self.n += n;
            while self.n >= 8 {
                self.out.push(self.acc as u8);
                self.acc >>= 8;
                self.n -= 8;
            }
        }
        fn put_rev(&mut self, code: u32, len: u32) {
            let mut r = 0;
            for i in 0..len {
                r |= ((code >> i) & 1) << (len - 1 - i);
            }
            self.put(r, len);
        }
        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.out.push(self.acc as u8);
            }
            self.out
        }
    }
    fn fixed_lit(w: &mut BitW, s: u32) {
        match s {
            0..=143 => w.put_rev(0x30 + s, 8),
            144..=255 => w.put_rev(0x190 + s - 144, 9),
            256..=279 => w.put_rev(s - 256, 7),
            _ => w.put_rev(0xC0 + s - 280, 8),
        }
    }
    fn deflate_fixed(data: &[u8]) -> Vec<u8> {
        let mut w = BitW { out: Vec::new(), acc: 0, n: 0 };
        w.put(1, 1);
        w.put(1, 2);
        let mut i = 0;
        while i < data.len() {
            let (mut best_len, mut best_d) = (0, 0);
            for d in 1..=i.min(32768) {
                let mut l = 0;
                while l < 258 && i + l < data.len() && data[i + l] == data[i + l - d] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_d = d;
                }
            }
            if best_len >= 3 {
                let li = LEN_BASE.iter().rposition(|&b| b as usize <= best_len).unwrap();
                fixed_lit(&mut w, 257 + li as u32);
                w.put((best_len - LEN_BASE[li] as usize) as u32, LEN_EXTRA[li] as u32);
                let di = DIST_BASE.iter().rposition(|&b| b as usize <= best_d).unwrap();
                w.put_rev(di as u32, 5);
                w.put((best_d - DIST_BASE[di] as usize) as u32, DIST_EXTRA[di] as u32);
                i += best_len;
            } else {
                fixed_lit(&mut w, data[i] as u32);
                i += 1;
            }
        }
        fixed_lit(&mut w, 256);
        w.finish()
    }

    fn stored(data: &[u8]) -> Vec<u8> {
        let mut v = vec![1u8];
        v.extend_from_slice(&(data.len() as u16).to_le_bytes());
        v.extend_from_slice(&(!(data.len() as u16)).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    fn make_zip(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut z = Vec::new();
        let mut cd = Vec::new();
        for (name, data, deflate) in members {
            let comp = if *deflate { deflate_fixed(data) } else { data.to_vec() };
            let off = z.len() as u32;
            let crc = crc32(data);
            let method: u16 = if *deflate { 8 } else { 0 };
            let mut lh = vec![0x50, 0x4b, 3, 4, 20, 0, 0, 0];
            lh.extend_from_slice(&method.to_le_bytes());
            lh.extend_from_slice(&[0; 4]);
            lh.extend_from_slice(&crc.to_le_bytes());
            lh.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            lh.extend_from_slice(&(data.len() as u32).to_le_bytes());
            lh.extend_from_slice(&(name.len() as u16).to_le_bytes());
            lh.extend_from_slice(&[0, 0]);
            lh.extend_from_slice(name.as_bytes());
            z.extend_from_slice(&lh);
            z.extend_from_slice(&comp);
            let mut c = vec![0x50, 0x4b, 1, 2, 20, 0, 20, 0, 0, 0];
            c.extend_from_slice(&method.to_le_bytes());
            c.extend_from_slice(&[0; 4]);
            c.extend_from_slice(&crc.to_le_bytes());
            c.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(name.len() as u16).to_le_bytes());
            c.extend_from_slice(&[0; 12]);
            c.extend_from_slice(&off.to_le_bytes());
            c.extend_from_slice(name.as_bytes());
            cd.extend_from_slice(&c);
        }
        let cd_off = z.len() as u32;
        z.extend_from_slice(&cd);
        z.extend_from_slice(&[0x50, 0x4b, 5, 6, 0, 0, 0, 0]);
        z.extend_from_slice(&(members.len() as u16).to_le_bytes());
        z.extend_from_slice(&(members.len() as u16).to_le_bytes());
        z.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        z.extend_from_slice(&cd_off.to_le_bytes());
        z.extend_from_slice(&[0, 0]);
        z
    }

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32_update(crc32(b"1234"), b"56789"), 0xCBF4_3926);
    }

    #[test]
    fn inflate_stored_and_fixed() {
        let data: Vec<u8> = b"hello hello hello, deflate! ".iter().cycle().take(5000).copied().collect();
        assert_eq!(inflate(&stored(&data[..1000])).unwrap(), &data[..1000]);
        let enc = deflate_fixed(&data);
        assert!(enc.len() < data.len() / 4);
        assert_eq!(inflate(&enc).unwrap(), data);
        // overlapping copies (distance 1) and all byte values
        let mut d2 = vec![7u8; 600];
        d2.extend(0..=255u8);
        assert_eq!(inflate(&deflate_fixed(&d2)).unwrap(), d2);
        // empty
        assert_eq!(inflate(&deflate_fixed(b"")).unwrap(), b"");
    }

    #[test]
    fn inflate_python_zlib_vectors() {
        // fixed-Huffman block from zlib level 9
        let fixed = hex(
            "0bc94855282ccd4cce56482aca2fcf5348cbaf50c82acd2d2856c82f4b2d5228014ae72456552aa4e4a7eb29848c2a1e553caa98da8a191899985958d9d83938b9b87978f9f8050485844544c5c42524a5a46564e5e4151495945554d5d43534b5b47574f5f40d0c8d8c4d4ccdcc2d2cadac6d6cedec1d1c9d9c5d5cdddc3d3cbdbc7d7cfdfc030283824342c3c22322a3a26362e3e2131293925352d3d23332b3b27372f3f20b0a8b8a4b4acbca2b2aabaa6b6aebea1b1a9b9a5b5adbda3b3abbba7b7afbfa274c9c3479cad469d367cc9c357bcedc79f3172c5cb478c9d265cb57ac5cb57acdda75eb376cdcb479cbd66ddb77ecdcb57bcfde7dfb0f1c3c74f8c8d163c74f9c3c75faccd973e72f5cbc74f9cad56bd76fdcbc75fbcedd7bf71f3c7cf4f8c9d367cf5fbc7cf5facddb77ef3f7cfcf4f9cbd76fdf7ffcfcf5fbcfdf7fff81ce43430a19a93939f92864797e514e0a00",
        );
        let mut want: Vec<u8> = b"The quick brown fox jumps over the lazy dog. ".repeat(20);
        want.extend(0..=255u8);
        want.extend_from_slice(b"abcabcabcabcabcabc hello hello hello world");
        assert_eq!(inflate(&fixed).unwrap(), want);
        // dynamic-Huffman block
        let dynamic = hex(
            "7dd4cb6d03310c45d17daa70091629ea936e92b117018c14e0ea636fe611d08d66e90751e211e5c7cfeffd72fdbc7cbdbfefd7771cc7edf6fae9f27c7e3cde6159c37286b686f50c7d0de7195628dbce34d6d4e24cdb9abad6763893f61d6bda74e4b9a643dd168072a520652a5dc8aa2b06ad6e8ac12ba5e0e5e9dc00d6d26a109bd22e40e6a96d30eb1a1f03b3a2bd0dd042c50dd0a6aeda00ad8adc006da6e105b59af606b5998a835aa4be416d2805b426720734d3ac38a00d1577408bf42c01cd74230e6843efc701adc9d4e96da6c600ada4d584a6eb7652535c41ad8aa5829aabef0a6a25ad06b5b4354d9a46a902da904a05b49efe2a01ada7d580d6d3de731bc7755b3ccaf66861dbc6c2b72c5177a611db0b89b6bdcee8db6188b11da598ff0de21f",
        );
        let want: String = (0..60).map(|i| format!("line {i}: aaaaabbbbcccdde {} zz\n", i * i % 97)).collect();
        let got = inflate(&dynamic).unwrap();
        assert_eq!(crc32(&got), 0xbf72_f1e5);
        assert_eq!(got, want.as_bytes());
    }

    #[test]
    fn inflate_rejects_garbage() {
        assert!(inflate(&[0x07]).is_err()); // reserved block type
        assert!(inflate(&[0x01, 0x05, 0x00, 0x00, 0x00]).is_err()); // bad NLEN
        let enc = deflate_fixed(b"some data some data some data");
        assert!(inflate(&enc[..enc.len() - 2]).is_err());
    }

    #[test]
    fn gzip_and_zlib() {
        let gz = hex("1f8b0800000000000213cb48cdc9c95748afca2c5028cf2fca49d151c84013e10200066931e423000000");
        assert_eq!(gunzip(&gz).unwrap(), b"hello gzip world, hello gzip world\n");
        assert_eq!(inflate(&gz).unwrap(), b"hello gzip world, hello gzip world\n");
        let mut two = gz.clone();
        two.extend_from_slice(&gz);
        assert_eq!(gunzip(&two).unwrap().len(), 70);
        let mut bad = gz.clone();
        let n = bad.len();
        bad[n - 8] ^= 1;
        assert!(gunzip(&bad).is_err());
        // zlib wrapper around a fixed block
        let body = deflate_fixed(b"zlib zlib zlib");
        let mut z = vec![0x78, 0x01];
        z.extend_from_slice(&body);
        let (mut a, mut b) = (1u32, 0u32);
        for &x in b"zlib zlib zlib" {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        assert_eq!(inflate_zlib(&z).unwrap(), b"zlib zlib zlib");
    }

    #[test]
    fn zip_stored_and_deflate() {
        let big: Vec<u8> = (0..20000u32).map(|i| (i % 251) as u8 ^ (i / 1000) as u8).collect();
        let z = make_zip(&[("a.txt", b"stored member", false), ("dir/b.bin", &big, true), ("dir/", b"", false)]);
        let a = ZipArchive::open(z).unwrap();
        assert_eq!(a.names(), vec!["a.txt", "dir/b.bin", "dir/"]);
        assert_eq!(a.read("a.txt").unwrap(), b"stored member");
        assert_eq!(a.read("dir/b.bin").unwrap(), big);
        assert!(a.entry("dir/").unwrap().is_dir());
        assert!(a.read("missing").is_err());
        assert_eq!(a.entry("dir/b.bin").unwrap().size, 20000);
    }

    #[test]
    fn zip_crc_mismatch_detected() {
        let mut z = make_zip(&[("x", b"payload", false)]);
        let p = z.windows(7).position(|w| w == b"payload").unwrap();
        z[p] = b'P';
        assert!(ZipArchive::open(z).unwrap().read("x").is_err());
    }

    fn tar_header(name: &str, size: usize, kind: u8) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&c| c as u32).sum();
        h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        h
    }

    #[test]
    fn untar_ustar_and_gnu_long_names() {
        let mut t = Vec::new();
        let add = |t: &mut Vec<u8>, name: &str, data: &[u8], kind: u8| {
            t.extend(tar_header(name, data.len(), kind));
            t.extend_from_slice(data);
            t.resize(t.len().div_ceil(512) * 512, 0);
        };
        add(&mut t, "model/", b"", b'5');
        add(&mut t, "model/config.ini", b"[df]\nsr=48000\n", b'0');
        let long = "model/".to_string() + &"x".repeat(150) + ".onnx";
        add(&mut t, "././@LongLink", format!("{long}\0").as_bytes(), b'L');
        add(&mut t, "model/xxxx", &[1, 2, 3, 4, 5], b'0');
        t.extend(vec![0u8; 1024]);
        let m = untar(&t);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], ("model/config.ini".to_string(), b"[df]\nsr=48000\n".to_vec()));
        assert_eq!(m[1].0, long);
        assert_eq!(m[1].1, vec![1, 2, 3, 4, 5]);
    }
}
