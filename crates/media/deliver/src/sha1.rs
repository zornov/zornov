// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SHA-1 (RFC 3174) and base64, for the hashes in packing lists. SHA-1 is
//! what SMPTE ST 429-8 and ST 2067-2 specify for asset integrity; it is
//! not used here for anything security-sensitive.

pub struct Sha1 {
    h: [u32; 5],
    buf: [u8; 64],
    n: usize,
    len: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub fn new() -> Self {
        Sha1 { h: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0], buf: [0; 64], n: 0, len: 0 }
    }

    fn block(&mut self, b: &[u8]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b_, mut c, mut d, mut e] = self.h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b_ & c) | (!b_ & d), 0x5a827999),
                20..=39 => (b_ ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b_ & c) | (b_ & d) | (c & d), 0x8f1bbcdc),
                _ => (b_ ^ c ^ d, 0xca62c1d6u32),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b_.rotate_left(30);
            b_ = a;
            a = t;
        }
        for (h, v) in self.h.iter_mut().zip([a, b_, c, d, e]) {
            *h = h.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if self.n > 0 {
            let k = (64 - self.n).min(data.len());
            self.buf[self.n..self.n + k].copy_from_slice(&data[..k]);
            self.n += k;
            data = &data[k..];
            if self.n == 64 {
                let b = self.buf;
                self.block(&b);
                self.n = 0;
            }
        }
        while data.len() >= 64 {
            self.block(&data[..64]);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.n = data.len();
        }
    }

    pub fn finish(mut self) -> [u8; 20] {
        let bits = self.len * 8;
        let mut pad = vec![0x80u8];
        while (self.n + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_be_bytes());
        let len = self.len;
        self.update(&pad);
        self.len = len;
        let mut out = [0u8; 20];
        for i in 0..5 {
            out[4 * i..4 * i + 4].copy_from_slice(&self.h[i].to_be_bytes());
        }
        out
    }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// SHA-1 of a file, base64 (the PKL `Hash`), and its size.
pub fn file_hash(path: &std::path::Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        size += n as u64;
    }
    Ok((base64(&h.finish()), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        let hex = |d: [u8; 20]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut h = Sha1::new();
        h.update(b"abc");
        assert_eq!(hex(h.finish()), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let mut h = Sha1::new();
        h.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(hex(h.finish()), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(&[0xa9, 0x99, 0x3e, 0x36]), "qZk+Ng==");
    }
}
