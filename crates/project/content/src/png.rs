// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! PNG writer with dynamic Huffman DEFLATE.
//!
//! `studio-media` can read PNGs and write them with fixed Huffman codes;
//! for a library whose core bundle has to stay under 300 MB the extra
//! 15–30 % from dynamic Huffman tables is worth the code below. Filtering
//! picks the per-row filter with the smallest absolute residual sum, as
//! libpng does.

use studio_media::png::{adler32, crc32};
use studio_media::rgba::RgbaImage;

/// Encodes an RGBA8 picture as PNG. Fully opaque pictures are written as
/// RGB (colour type 2) to save a quarter of the raw bytes.
pub fn encode(img: &RgbaImage) -> Vec<u8> {
    let opaque = img.pixels.chunks_exact(4).all(|p| p[3] == 255);
    let bpp = if opaque { 3 } else { 4 };
    let w = img.width as usize;
    let h = img.height as usize;
    let stride = w * bpp;
    let mut packed: Vec<u8>;
    let src: &[u8] = if opaque {
        packed = Vec::with_capacity(w * h * 3);
        for p in img.pixels.chunks_exact(4) {
            packed.extend_from_slice(&p[..3]);
        }
        &packed
    } else {
        &img.pixels
    };
    let mut raw = Vec::with_capacity((stride + 1) * h);
    let zero = vec![0u8; stride];
    let mut trial = vec![0u8; stride];
    let mut best = vec![0u8; stride];
    for y in 0..h {
        let row = &src[y * stride..(y + 1) * stride];
        let prev = if y == 0 { &zero[..] } else { &src[(y - 1) * stride..y * stride] };
        let mut best_score = u64::MAX;
        let mut best_kind = 0u8;
        for kind in 0..5u8 {
            filter_row(kind, row, prev, bpp, &mut trial);
            let score: u64 = trial.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum();
            if score < best_score {
                best_score = score;
                best_kind = kind;
                std::mem::swap(&mut best, &mut trial);
            }
        }
        raw.push(best_kind);
        raw.extend_from_slice(&best);
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, if opaque { 2 } else { 6 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Writes `img` as a PNG file (atomically via a `.part` file).
pub fn write(img: &RgbaImage, path: &std::path::Path) -> std::io::Result<()> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, encode(img))?;
    std::fs::rename(tmp, path)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn filter_row(kind: u8, row: &[u8], prev: &[u8], bpp: usize, out: &mut [u8]) {
    for i in 0..row.len() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        let b = prev[i];
        let c = if i >= bpp { prev[i - bpp] } else { 0 };
        let p = match kind {
            0 => 0,
            1 => a,
            2 => b,
            3 => ((a as u16 + b as u16) / 2) as u8,
            _ => paeth(a, b, c),
        };
        out[i] = row[i].wrapping_sub(p);
    }
}

/// zlib stream (header, DEFLATE, Adler-32).
pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0xda];
    deflate(data, &mut out);
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter<'_> {
    fn put(&mut self, value: u32, bits: u32) {
        self.acc |= (value as u64) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// Huffman codes are sent most significant bit first.
    fn put_code(&mut self, code: u32, len: u32) {
        let mut rev = 0u32;
        for i in 0..len {
            rev |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.put(rev, len);
    }

    fn finish(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc = 0;
            self.n = 0;
        }
    }
}

const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

fn len_code(len: usize) -> (usize, u32, u32) {
    let mut i = 28;
    while LEN_BASE[i] as usize > len {
        i -= 1;
    }
    (257 + i, (len - LEN_BASE[i] as usize) as u32, LEN_EXTRA[i] as u32)
}

fn dist_code(dist: usize) -> (usize, u32, u32) {
    let mut i = 29;
    while DIST_BASE[i] as usize > dist {
        i -= 1;
    }
    (i, (dist - DIST_BASE[i] as usize) as u32, DIST_EXTRA[i] as u32)
}

/// A literal (`dist == 0`) or a back-reference.
#[derive(Clone, Copy)]
struct Token {
    lit_or_len: u16,
    dist: u16,
}

/// Length-limited Huffman code lengths (zlib's overflow fix on top of a
/// plain Huffman tree), then canonical codes.
fn huffman(freq: &[u32], max_len: u32) -> (Vec<u32>, Vec<u32>) {
    let n = freq.len();
    let mut lens = vec![0u32; n];
    let used: Vec<usize> = (0..n).filter(|&i| freq[i] > 0).collect();
    if used.is_empty() {
        return (lens, vec![0; n]);
    }
    if used.len() == 1 {
        lens[used[0]] = 1;
    } else {
        // Build the tree with a simple two-queue merge on sorted leaves.
        #[derive(Clone)]
        struct Node {
            w: u64,
            depth_leaves: Vec<usize>,
        }
        let mut leaves: Vec<Node> = used.iter().map(|&i| Node { w: freq[i] as u64, depth_leaves: vec![i] }).collect();
        leaves.sort_by_key(|l| l.w);
        let mut merged: std::collections::VecDeque<Node> = std::collections::VecDeque::new();
        let mut li = 0;
        let pop = |leaves: &Vec<Node>, li: &mut usize, merged: &mut std::collections::VecDeque<Node>| -> Node {
            let take_leaf = match (leaves.get(*li), merged.front()) {
                (Some(l), Some(m)) => l.w <= m.w,
                (Some(_), None) => true,
                _ => false,
            };
            if take_leaf {
                *li += 1;
                leaves[*li - 1].clone()
            } else {
                merged.pop_front().unwrap()
            }
        };
        let total = leaves.len();
        for _ in 0..total - 1 {
            let a = pop(&leaves, &mut li, &mut merged);
            let b = pop(&leaves, &mut li, &mut merged);
            for &s in a.depth_leaves.iter().chain(b.depth_leaves.iter()) {
                lens[s] += 1;
            }
            let mut dl = a.depth_leaves;
            dl.extend(b.depth_leaves);
            merged.push_back(Node { w: a.w + b.w, depth_leaves: dl });
        }
        // Fix lengths above the limit (zlib gen_bitlen overflow handling).
        let mut count = vec![0u32; (max_len + 1) as usize];
        let mut overflow = 0u32;
        for &s in &used {
            if lens[s] > max_len {
                overflow += lens[s] - max_len;
                lens[s] = max_len;
            }
            count[lens[s] as usize] += 1;
        }
        // Each overflow step above moved one leaf up; restore Kraft equality.
        let mut kraft: i64 = used.iter().map(|&s| 1i64 << (max_len - lens[s])).sum();
        let limit = 1i64 << max_len;
        if overflow > 0 || kraft > limit {
            while kraft > limit {
                let mut bits = max_len - 1;
                while count[bits as usize] == 0 {
                    bits -= 1;
                }
                count[bits as usize] -= 1;
                count[(bits + 1) as usize] += 2;
                count[max_len as usize] -= 1;
                kraft -= 1i64 << (max_len - bits - 1);
            }
            // Reassign lengths: least frequent symbols get the longest codes.
            let mut order = used.clone();
            order.sort_by_key(|&s| (freq[s], s));
            let mut idx = 0;
            for bits in (1..=max_len).rev() {
                for _ in 0..count[bits as usize] {
                    lens[order[idx]] = bits;
                    idx += 1;
                }
            }
        }
    }
    (lens.clone(), canonical(&lens))
}

fn canonical(lens: &[u32]) -> Vec<u32> {
    let max = lens.iter().copied().max().unwrap_or(0) as usize;
    let mut bl_count = vec![0u32; max + 2];
    for &l in lens {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }
    let mut next = vec![0u32; max + 2];
    let mut code = 0u32;
    for bits in 1..=max {
        code = (code + bl_count[bits - 1]) << 1;
        next[bits] = code;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                0
            } else {
                let c = next[l as usize];
                next[l as usize] += 1;
                c
            }
        })
        .collect()
}

/// LZ77 with hash chains, then one dynamic-Huffman block per 64K tokens.
fn deflate(data: &[u8], out: &mut Vec<u8>) {
    const HASH_BITS: u32 = 16;
    const WINDOW: usize = 32_768;
    const MAX_CHAIN: usize = 48;
    const BLOCK_TOKENS: usize = 1 << 16;
    let mut head = vec![u32::MAX; 1 << HASH_BITS];
    let mut prev = vec![u32::MAX; data.len().max(1)];
    let hash = |i: usize| -> usize {
        let v = u32::from(data[i]) << 16 | u32::from(data[i + 1]) << 8 | u32::from(data[i + 2]);
        (v.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize
    };
    let mut tokens: Vec<Token> = Vec::with_capacity(BLOCK_TOKENS);
    let mut bw = BitWriter { out, acc: 0, n: 0 };
    let mut i = 0usize;
    let n = data.len();
    let insert = |i: usize, head: &mut Vec<u32>, prev: &mut Vec<u32>| {
        if i + 2 < n {
            let h = hash(i);
            prev[i] = head[h];
            head[h] = i as u32;
        }
    };
    while i < n {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        if i + 2 < n {
            let mut cand = head[hash(i)];
            let mut chain = 0;
            let max_len = (n - i).min(258);
            while cand != u32::MAX && chain < MAX_CHAIN {
                let c = cand as usize;
                if i - c > WINDOW {
                    break;
                }
                if data[c + best_len.min(max_len - 1)] == data[i + best_len.min(max_len - 1)] {
                    let mut l = 0;
                    while l < max_len && data[c + l] == data[i + l] {
                        l += 1;
                    }
                    if l > best_len {
                        best_len = l;
                        best_dist = i - c;
                        if l == max_len {
                            break;
                        }
                    }
                }
                cand = prev[c];
                chain += 1;
            }
        }
        if best_len >= 3 {
            tokens.push(Token { lit_or_len: best_len as u16, dist: best_dist as u16 });
            for k in 0..best_len {
                insert(i + k, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            tokens.push(Token { lit_or_len: data[i] as u16, dist: 0 });
            insert(i, &mut head, &mut prev);
            i += 1;
        }
        if tokens.len() >= BLOCK_TOKENS {
            write_block(&mut bw, &tokens, false);
            tokens.clear();
        }
    }
    write_block(&mut bw, &tokens, true);
    bw.finish();
}

fn write_block(bw: &mut BitWriter, tokens: &[Token], last: bool) {
    let mut lit_freq = vec![0u32; 286];
    let mut dist_freq = vec![0u32; 30];
    for t in tokens {
        if t.dist == 0 {
            lit_freq[t.lit_or_len as usize] += 1;
        } else {
            lit_freq[len_code(t.lit_or_len as usize).0] += 1;
            dist_freq[dist_code(t.dist as usize).0] += 1;
        }
    }
    lit_freq[256] += 1;
    if dist_freq.iter().all(|&f| f == 0) {
        dist_freq[0] = 1; // at least one distance code must exist
    }
    let (lit_len, lit_code) = huffman(&lit_freq, 15);
    let (dist_len, dist_codes) = huffman(&dist_freq, 15);
    let hlit = (257..=286).rev().find(|&n| lit_len[n - 1] > 0).unwrap_or(257).max(257);
    let hdist = (1..=30).rev().find(|&n| dist_len[n - 1] > 0).unwrap_or(1);
    // Run-length encode the code lengths with symbols 16/17/18.
    let mut all: Vec<u32> = lit_len[..hlit].to_vec();
    all.extend_from_slice(&dist_len[..hdist]);
    let mut cl_syms: Vec<(u32, u32, u32)> = Vec::new(); // (symbol, extra value, extra bits)
    let mut j = 0;
    while j < all.len() {
        let v = all[j];
        let mut run = 1;
        while j + run < all.len() && all[j + run] == v {
            run += 1;
        }
        if v == 0 && run >= 3 {
            let r = run.min(138);
            if r >= 11 {
                cl_syms.push((18, (r - 11) as u32, 7));
            } else {
                cl_syms.push((17, (r - 3) as u32, 3));
            }
            j += r;
        } else if v != 0 && run >= 4 {
            cl_syms.push((v, 0, 0));
            let mut rest = run - 1;
            j += 1;
            while rest >= 3 {
                let r = rest.min(6);
                cl_syms.push((16, (r - 3) as u32, 2));
                rest -= r;
                j += r;
            }
        } else {
            cl_syms.push((v, 0, 0));
            j += 1;
        }
    }
    let mut cl_freq = vec![0u32; 19];
    for (s, _, _) in &cl_syms {
        cl_freq[*s as usize] += 1;
    }
    let (cl_len, cl_code) = huffman(&cl_freq, 7);
    let hclen = (4..=19).rev().find(|&n| cl_len[CL_ORDER[n - 1]] > 0).unwrap_or(4);
    bw.put(last as u32, 1);
    bw.put(2, 2);
    bw.put((hlit - 257) as u32, 5);
    bw.put((hdist - 1) as u32, 5);
    bw.put((hclen - 4) as u32, 4);
    for &o in CL_ORDER.iter().take(hclen) {
        bw.put(cl_len[o], 3);
    }
    for (s, extra, bits) in &cl_syms {
        bw.put_code(cl_code[*s as usize], cl_len[*s as usize]);
        if *bits > 0 {
            bw.put(*extra, *bits);
        }
    }
    for t in tokens {
        if t.dist == 0 {
            let s = t.lit_or_len as usize;
            bw.put_code(lit_code[s], lit_len[s]);
        } else {
            let (ls, lextra, lbits) = len_code(t.lit_or_len as usize);
            bw.put_code(lit_code[ls], lit_len[ls]);
            if lbits > 0 {
                bw.put(lextra, lbits);
            }
            let (ds, dextra, dbits) = dist_code(t.dist as usize);
            bw.put_code(dist_codes[ds], dist_len[ds]);
            if dbits > 0 {
                bw.put(dextra, dbits);
            }
        }
    }
    bw.put_code(lit_code[256], lit_len[256]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_media::deflate::zlib_decompress;

    fn noisy(w: u32, h: u32, alpha: bool) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        let mut x = 7u32;
        for (i, p) in img.pixels.chunks_exact_mut(4).enumerate() {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let v = (x >> 16) as u8;
            p[0] = v;
            p[1] = (i % 251) as u8;
            p[2] = ((i / 7) % 200) as u8;
            p[3] = if alpha { if i % 3 == 0 { 0 } else { 200 } } else { 255 };
        }
        img
    }

    #[test]
    fn zlib_round_trips_many_shapes() {
        let mut samples: Vec<Vec<u8>> = vec![vec![], b"a".to_vec(), b"abcabcabcabcabcabcabc".to_vec(), vec![0u8; 300_000]];
        let mut x = 1u32;
        samples.push((0..200_000).map(|_| { x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345); (x >> 16) as u8 }).collect());
        samples.push((0..90_000u32).map(|i| (i % 251) as u8).collect());
        samples.push((0..100_000u32).map(|i| if i % 1000 < 3 { (i % 7) as u8 } else { 0 }).collect());
        for s in samples {
            let z = zlib(&s);
            assert_eq!(zlib_decompress(&z, 1 << 24).unwrap(), s);
        }
        assert!(zlib(&[0u8; 300_000]).len() < 400);
    }

    #[test]
    fn png_round_trips_through_media_decoder() {
        for alpha in [true, false] {
            let img = noisy(77, 41, alpha);
            let bytes = encode(&img);
            let back = studio_media::png::decode(&bytes).unwrap();
            assert_eq!(back.width, 77);
            assert_eq!(back.height, 41);
            assert_eq!(back.pixels, img.pixels);
        }
    }

    #[test]
    fn dynamic_codes_beat_fixed_codes() {
        let mut img = RgbaImage::new(256, 128);
        for (i, p) in img.pixels.chunks_exact_mut(4).enumerate() {
            let x = (i % 256) as u8;
            let y = (i / 256) as u8;
            p.copy_from_slice(&[x, y, x ^ y, if x > 40 { 255 } else { 0 }]);
        }
        let ours = encode(&img).len();
        let fixed = studio_media::png::encode(&img).len();
        assert!(ours < fixed, "ours {ours} fixed {fixed}");
    }
}
