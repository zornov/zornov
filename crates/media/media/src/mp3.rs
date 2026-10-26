// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MP3 (MPEG-1 / MPEG-2 / MPEG-2.5 Layer III) decoder.
//!
//! Frame headers and side information, the bit reservoir, MPEG-1 and LSF
//! scalefactors, Huffman decoding, requantisation, mid/side and intensity
//! stereo, short-block reordering, alias reduction, IMDCT with overlap-add,
//! frequency inversion and the polyphase synthesis filterbank. ID3v2 tags
//! are skipped; a Xing/Info frame is skipped, and its LAME tag's encoder
//! delay and padding are trimmed (gapless). Layer I/II files are rejected.

use crate::mp3_tables::*;
use crate::wav::Wav;
use std::f64::consts::PI;
use std::sync::OnceLock;

const BITRATES_V1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const BITRATES_V2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
const RATES: [[u32; 3]; 3] = [[44100, 48000, 32000], [22050, 24000, 16000], [11025, 12000, 8000]];
const PRETAB: [u8; 22] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 3, 2, 0];
const SLEN: [(u32, u32); 16] = [(0, 0), (0, 1), (0, 2), (0, 3), (3, 0), (1, 1), (1, 2), (1, 3), (2, 1), (2, 2), (2, 3), (3, 1), (3, 2), (3, 3), (4, 2), (4, 3)];
const LINBITS: [u32; 32] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 6, 8, 10, 13, 4, 5, 6, 7, 8, 9, 11, 13];
/// MPEG-2 LSF scalefactor counts [table][block kind: long, short, mixed][part].
const NR_OF_SFB: [[[usize; 4]; 3]; 6] = [
    [[6, 5, 5, 5], [9, 9, 9, 9], [6, 9, 9, 9]],
    [[6, 5, 7, 3], [9, 9, 12, 6], [6, 9, 12, 6]],
    [[11, 10, 0, 0], [18, 18, 0, 0], [15, 18, 0, 0]],
    [[7, 7, 7, 0], [12, 12, 12, 0], [6, 15, 12, 0]],
    [[6, 6, 6, 3], [12, 9, 9, 6], [6, 12, 9, 6]],
    [[8, 8, 5, 0], [15, 12, 9, 0], [6, 18, 9, 0]],
];

#[derive(Clone, Copy, Debug)]
struct Header {
    /// 0 = MPEG-1, 1 = MPEG-2, 2 = MPEG-2.5.
    version: usize,
    crc: bool,
    sample_rate: u32,
    rate_index: usize,
    /// 3 = mono.
    mode: u8,
    mode_ext: u8,
    frame_len: usize,
}

impl Header {
    fn parse(b: &[u8]) -> Option<Header> {
        if b.len() < 4 || b[0] != 0xFF || b[1] & 0xE0 != 0xE0 {
            return None;
        }
        let version = match (b[1] >> 3) & 3 {
            3 => 0,
            2 => 1,
            0 => 2,
            _ => return None,
        };
        if (b[1] >> 1) & 3 != 1 {
            return None; // not Layer III
        }
        let bi = (b[2] >> 4) as usize;
        let si = ((b[2] >> 2) & 3) as usize;
        if bi == 0 || bi == 15 || si == 3 {
            return None;
        }
        let bitrate = if version == 0 { BITRATES_V1[bi] } else { BITRATES_V2[bi] } * 1000;
        let sample_rate = RATES[version][si];
        let padding = (b[2] >> 1) & 1 == 1;
        let per = if version == 0 { 144 } else { 72 };
        let frame_len = (per * bitrate / sample_rate) as usize + padding as usize;
        Some(Header { version, crc: b[1] & 1 == 0, sample_rate, rate_index: version * 3 + si, mode: b[3] >> 6, mode_ext: (b[3] >> 4) & 3, frame_len })
    }

    fn channels(&self) -> usize {
        if self.mode == 3 { 1 } else { 2 }
    }

    fn granules(&self) -> usize {
        if self.version == 0 { 2 } else { 1 }
    }

    fn side_len(&self) -> usize {
        match (self.version == 0, self.channels()) {
            (true, 1) => 17,
            (true, _) => 32,
            (false, 1) => 9,
            (false, _) => 17,
        }
    }
}

/// MSB-first bit reader.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Self {
        Bits { d, pos: 0 }
    }
    fn bit(&mut self) -> u32 {
        let byte = self.d.get(self.pos >> 3).copied().unwrap_or(0);
        let v = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        v as u32
    }
    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }
}

#[derive(Clone, Copy, Default)]
struct Granule {
    part2_3: usize,
    big_values: usize,
    global_gain: i32,
    sf_compress: u32,
    switching: bool,
    block_type: u8,
    mixed: bool,
    table: [usize; 3],
    subblock_gain: [i32; 3],
    region0: usize,
    region1: usize,
    preflag: bool,
    sf_scale: bool,
    count1_b: bool,
}

struct SideInfo {
    main_data_begin: usize,
    scfsi: [[bool; 4]; 2],
    gr: [[Granule; 2]; 2],
}

fn read_side(h: &Header, s: &[u8]) -> SideInfo {
    let mut b = Bits::new(s);
    let nch = h.channels();
    let v1 = h.version == 0;
    let main_data_begin = b.bits(if v1 { 9 } else { 8 }) as usize;
    b.bits(match (v1, nch) {
        (true, 1) => 5,
        (true, _) => 3,
        (false, 1) => 1,
        (false, _) => 2,
    });
    let mut scfsi = [[false; 4]; 2];
    if v1 {
        for c in scfsi.iter_mut().take(nch) {
            for s in c.iter_mut() {
                *s = b.bit() == 1;
            }
        }
    }
    let mut gr = [[Granule::default(); 2]; 2];
    for g in gr.iter_mut().take(h.granules()) {
        for x in g.iter_mut().take(nch) {
            x.part2_3 = b.bits(12) as usize;
            x.big_values = (b.bits(9) as usize).min(288);
            x.global_gain = b.bits(8) as i32;
            x.sf_compress = b.bits(if v1 { 4 } else { 9 });
            x.switching = b.bit() == 1;
            if x.switching {
                x.block_type = b.bits(2) as u8;
                x.mixed = b.bit() == 1;
                x.table[0] = b.bits(5) as usize;
                x.table[1] = b.bits(5) as usize;
                for s in &mut x.subblock_gain {
                    *s = b.bits(3) as i32;
                }
                x.region0 = if x.block_type == 2 && !x.mixed { 8 } else { 7 };
                x.region1 = 36;
            } else {
                for t in &mut x.table {
                    *t = b.bits(5) as usize;
                }
                x.region0 = b.bits(4) as usize;
                x.region1 = b.bits(3) as usize;
            }
            if v1 {
                x.preflag = b.bit() == 1;
            }
            x.sf_scale = b.bit() == 1;
            x.count1_b = b.bit() == 1;
        }
    }
    SideInfo { main_data_begin, scfsi, gr }
}

/// Huffman decoding tree: node pairs; a leaf is stored as `!value`.
struct Tree {
    nodes: Vec<[i32; 2]>,
}

impl Tree {
    fn new(codes: &[(u16, u8)]) -> Tree {
        let mut nodes = vec![[0i32; 2]];
        for (v, &(code, len)) in codes.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let mut n = 0usize;
            for i in (0..len).rev() {
                let bit = ((code as u32 >> i) & 1) as usize;
                if i == 0 {
                    nodes[n][bit] = !(v as i32);
                } else {
                    if nodes[n][bit] == 0 {
                        nodes.push([0, 0]);
                        nodes[n][bit] = (nodes.len() - 1) as i32;
                    }
                    n = nodes[n][bit] as usize;
                }
            }
        }
        Tree { nodes }
    }

    fn decode(&self, b: &mut Bits) -> usize {
        let mut n = 0usize;
        for _ in 0..24 {
            let next = self.nodes[n][b.bit() as usize];
            if next < 0 {
                return (!next) as usize;
            }
            if next == 0 {
                return 0;
            }
            n = next as usize;
        }
        0
    }
}

struct Trees {
    /// Index = table number (0..=15 plus 16 / 24 shared); `None` for empty tables.
    big: Vec<Option<(Tree, usize)>>,
    quad_a: Tree,
}

fn trees() -> &'static Trees {
    static T: OnceLock<Trees> = OnceLock::new();
    T.get_or_init(|| {
        let t = |c: &[(u16, u8)], wrap: usize| Some((Tree::new(c), wrap));
        let big = vec![
            None,
            t(&HUFF_1, 2),
            t(&HUFF_2, 3),
            t(&HUFF_3, 3),
            None,
            t(&HUFF_5, 4),
            t(&HUFF_6, 4),
            t(&HUFF_7, 6),
            t(&HUFF_8, 6),
            t(&HUFF_9, 6),
            t(&HUFF_10, 8),
            t(&HUFF_11, 8),
            t(&HUFF_12, 8),
            t(&HUFF_13, 16),
            None,
            t(&HUFF_15, 16),
            t(&HUFF_16, 16),
            t(&HUFF_24, 16),
        ];
        Trees { big, quad_a: Tree::new(&QUAD_A) }
    })
}

/// Long-block band edges and short-block widths for a header.
fn bands(h: &Header) -> (&'static [u16; 23], &'static [u16; 13]) {
    (&SFB_LONG[h.rate_index], &SFB_SHORT_WIDTH[h.rate_index])
}

/// Scalefactors of one granule/channel: long bands, and short [band][window].
#[derive(Clone, Copy)]
struct Scalefactors {
    l: [u8; 22],
    s: [[u8; 3]; 13],
}

impl Default for Scalefactors {
    fn default() -> Self {
        Scalefactors { l: [0; 22], s: [[0; 3]; 13] }
    }
}

fn read_scalefactors_v1(b: &mut Bits, g: &Granule, scfsi: &[bool; 4], gr: usize, prev: &Scalefactors) -> Scalefactors {
    let (slen1, slen2) = SLEN[g.sf_compress as usize & 15];
    let mut sf = Scalefactors::default();
    if g.switching && g.block_type == 2 {
        let first_short = if g.mixed {
            for l in sf.l.iter_mut().take(8) {
                *l = b.bits(slen1) as u8;
            }
            3
        } else {
            0
        };
        for sfb in first_short..12 {
            let n = if sfb < 6 { slen1 } else { slen2 };
            for w in 0..3 {
                sf.s[sfb][w] = b.bits(n) as u8;
            }
        }
    } else {
        let groups = [(0usize, 6usize), (6, 11), (11, 16), (16, 21)];
        for (gi, &(a, z)) in groups.iter().enumerate() {
            let n = if gi < 2 { slen1 } else { slen2 };
            for sfb in a..z {
                sf.l[sfb] = if gr == 1 && scfsi[gi] { prev.l[sfb] } else { b.bits(n) as u8 };
            }
        }
    }
    sf
}

/// LSF scalefactors; also returns the preflag and, for the intensity
/// channel, the largest value per band (illegal intensity positions).
fn read_scalefactors_v2(b: &mut Bits, g: &Granule, intensity_right: bool) -> (Scalefactors, bool, Scalefactors) {
    let sfc = g.sf_compress;
    let (slen, idx, preflag) = if intensity_right {
        let i = sfc >> 1;
        if i < 180 {
            ([i / 36, (i % 36) / 6, (i % 36) % 6, 0], 3, false)
        } else if i < 244 {
            let i = i - 180;
            ([(i % 64) >> 4, (i % 16) >> 2, i % 4, 0], 4, false)
        } else {
            let i = i - 244;
            ([i / 3, i % 3, 0, 0], 5, false)
        }
    } else if sfc < 400 {
        ([(sfc >> 4) / 5, (sfc >> 4) % 5, (sfc % 16) >> 2, sfc % 4], 0, false)
    } else if sfc < 500 {
        let i = sfc - 400;
        ([(i >> 2) / 5, (i >> 2) % 5, i % 4, 0], 1, false)
    } else {
        let i = sfc - 500;
        ([i / 3, i % 3, 0, 0], 2, true)
    };
    let kind = if g.switching && g.block_type == 2 { if g.mixed { 2 } else { 1 } } else { 0 };
    let nr = NR_OF_SFB[idx][kind];
    let mut flat = Vec::with_capacity(54);
    let mut max = Vec::with_capacity(54);
    for p in 0..4 {
        for _ in 0..nr[p] {
            flat.push(b.bits(slen[p]) as u8);
            max.push(((1u32 << slen[p]) - 1) as u8);
        }
    }
    let mut sf = Scalefactors::default();
    let mut mx = Scalefactors::default();
    let mut k = 0;
    let mut take = |sf: &mut u8, m: &mut u8| {
        *sf = flat.get(k).copied().unwrap_or(0);
        *m = max.get(k).copied().unwrap_or(0);
        k += 1;
    };
    match kind {
        0 => {
            for sfb in 0..21 {
                take(&mut sf.l[sfb], &mut mx.l[sfb]);
            }
        }
        _ => {
            let first = if kind == 2 {
                for sfb in 0..6 {
                    take(&mut sf.l[sfb], &mut mx.l[sfb]);
                }
                3
            } else {
                0
            };
            for sfb in first..12 {
                for w in 0..3 {
                    take(&mut sf.s[sfb][w], &mut mx.s[sfb][w]);
                }
            }
        }
    }
    (sf, preflag, mx)
}

/// Huffman-decodes one granule/channel into `is` (576 quantised values);
/// returns the number of lines up to the last possibly-nonzero one.
fn huffman(b: &mut Bits, g: &Granule, h: &Header, end: usize, is: &mut [i32; 576]) -> usize {
    *is = [0; 576];
    let tr = trees();
    let (long, short) = bands(h);
    let big = (g.big_values * 2).min(576);
    let (r1, r2) = if g.switching {
        let r1 = if g.block_type == 2 && !g.mixed { 3 * short.iter().take(3).map(|&w| w as usize).sum::<usize>() } else { long[8] as usize };
        (r1, 576)
    } else {
        let a = (g.region0 + 1).min(22);
        let z = (g.region0 + g.region1 + 2).min(22);
        (long[a] as usize, long[z] as usize)
    };
    let mut i = 0;
    while i < big {
        let tn = g.table[if i < r1 { 0 } else if i < r2 { 1 } else { 2 }];
        let slot = match tn {
            0..=15 => tn,
            16..=23 => 16,
            _ => 17,
        };
        let Some((tree, wrap)) = &tr.big[slot] else {
            is[i] = 0;
            is[i + 1] = 0;
            i += 2;
            continue;
        };
        let v = tree.decode(b);
        let (mut x, mut y) = ((v / wrap) as i32, (v % wrap) as i32);
        let lin = LINBITS[tn];
        if lin > 0 && x == 15 {
            x += b.bits(lin) as i32;
        }
        if x != 0 && b.bit() == 1 {
            x = -x;
        }
        if lin > 0 && y == 15 {
            y += b.bits(lin) as i32;
        }
        if y != 0 && b.bit() == 1 {
            y = -y;
        }
        is[i] = x;
        is[i + 1] = y;
        i += 2;
    }
    // count1 region: quadruples of -1 / 0 / 1.
    while i + 4 <= 576 && b.pos < end {
        let v = if g.count1_b { 15 - b.bits(4) as usize } else { tr.quad_a.decode(b) };
        let mut q = [((v >> 3) & 1) as i32, ((v >> 2) & 1) as i32, ((v >> 1) & 1) as i32, (v & 1) as i32];
        for x in &mut q {
            if *x != 0 && b.bit() == 1 {
                *x = -1;
            }
        }
        if b.pos > end {
            break; // the last quadruple ran past part2_3: discard it
        }
        is[i..i + 4].copy_from_slice(&q);
        i += 4;
    }
    b.pos = end;
    i
}

fn pow43() -> &'static [f64] {
    static T: OnceLock<Vec<f64>> = OnceLock::new();
    T.get_or_init(|| (0..8207).map(|i| (i as f64).powf(4.0 / 3.0)).collect())
}

/// Requantises into `xr` (bitstream order: long bands, then short bands
/// band-major / window / line).
fn requantize(g: &Granule, h: &Header, sf: &Scalefactors, preflag: bool, is: &[i32; 576], xr: &mut [f64; 576]) {
    let p43 = pow43();
    let (long, short) = bands(h);
    let mult = if g.sf_scale { 1.0 } else { 0.5 };
    let base = 2f64.powf(0.25 * (g.global_gain - 210) as f64);
    let q = |v: i32| -> f64 {
        let a = p43[(v.unsigned_abs() as usize).min(8206)];
        if v < 0 { -a } else { a }
    };
    let short_block = g.switching && g.block_type == 2;
    let long_end = if !short_block { 576 } else if g.mixed { long[if h.version == 0 { 8 } else { 6 }] as usize } else { 0 };
    let mut sfb = 0;
    for i in 0..long_end {
        while sfb < 21 && i >= long[sfb + 1] as usize {
            sfb += 1;
        }
        let pre = if preflag { PRETAB[sfb] } else { 0 };
        let e = -mult * (sf.l[sfb.min(21)] as f64 + pre as f64);
        xr[i] = if is[i] == 0 { 0.0 } else { q(is[i]) * base * 2f64.powf(e) };
    }
    if short_block {
        let mut i = long_end;
        let first = if g.mixed { 3 } else { 0 };
        for (b, &w) in short.iter().enumerate().skip(first) {
            for win in 0..3 {
                let s = sf.s[b.min(12)][win];
                let gain = base * 2f64.powf(-2.0 * g.subblock_gain[win] as f64 - mult * s as f64);
                for _ in 0..w {
                    if i >= 576 {
                        return;
                    }
                    xr[i] = if is[i] == 0 { 0.0 } else { q(is[i]) * gain };
                    i += 1;
                }
            }
        }
    }
}

/// Short-block lines in bitstream order → [subband][window][line] layout.
fn reorder(g: &Granule, h: &Header, xr: &mut [f64; 576]) {
    if !(g.switching && g.block_type == 2) {
        return;
    }
    let (long, short) = bands(h);
    let start_sfb = if g.mixed { 3 } else { 0 };
    let mut src = if g.mixed { long[if h.version == 0 { 8 } else { 6 }] as usize } else { 0 };
    let mut freq = 3 * short.iter().take(start_sfb).map(|&w| w as usize).sum::<usize>() / 3;
    let mut out = *xr;
    for &w in short.iter().skip(start_sfb) {
        let w = w as usize;
        for win in 0..3 {
            for k in 0..w {
                let f = freq + k;
                let dst = (f / 6) * 18 + win * 6 + f % 6;
                if src < 576 && dst < 576 {
                    out[dst] = xr[src];
                }
                src += 1;
            }
        }
        freq += w;
    }
    *xr = out;
}

/// Band-wise start/end lines of the short or long layout for stereo work.
fn stereo(h: &Header, g: &[Granule; 2], sf_r: &Scalefactors, max_r: Option<&Scalefactors>, nz_r: usize, xr: &mut [[f64; 576]; 2]) {
    let ms = h.mode == 1 && h.mode_ext & 2 != 0;
    let intensity = h.mode == 1 && h.mode_ext & 1 != 0;
    let mut is_from = 576usize;
    if intensity {
        is_from = intensity_stereo(h, &g[1], sf_r, max_r, nz_r, xr);
    }
    if ms {
        let r = std::f64::consts::FRAC_1_SQRT_2;
        for i in 0..is_from.min(576) {
            let (m, s) = (xr[0][i], xr[1][i]);
            xr[0][i] = (m + s) * r;
            xr[1][i] = (m - s) * r;
        }
    }
}

/// Applies intensity stereo above the right channel's last nonzero line;
/// returns the first line it applied to (mid/side covers the lines below).
fn intensity_stereo(h: &Header, g: &Granule, sf: &Scalefactors, max: Option<&Scalefactors>, nz: usize, xr: &mut [[f64; 576]; 2]) -> usize {
    let (long, short) = bands(h);
    // Last nonzero right-channel line.
    let last = (0..nz.min(576)).rev().find(|&i| xr[1][i] != 0.0).map_or(0, |i| i + 1);
    let lsf_io = if g.sf_compress & 1 == 1 { std::f64::consts::FRAC_1_SQRT_2 } else { 2f64.powf(-0.25) };
    let apply = |xr: &mut [[f64; 576]; 2], a: usize, z: usize, pos: u8, illegal: bool| {
        if illegal {
            return;
        }
        let (kl, kr) = if h.version == 0 {
            if pos >= 7 {
                return;
            }
            let t = (pos as f64 * PI / 12.0).tan();
            (t / (1.0 + t), 1.0 / (1.0 + t))
        } else if pos == 0 {
            (1.0, 1.0)
        } else if pos & 1 == 1 {
            (lsf_io.powf(((pos + 1) / 2) as f64), 1.0)
        } else {
            (1.0, lsf_io.powf((pos / 2) as f64))
        };
        for i in a..z.min(576) {
            let x = xr[0][i];
            xr[0][i] = x * kl;
            xr[1][i] = x * kr;
        }
    };
    if g.switching && g.block_type == 2 {
        // Short blocks (after reorder the layout is per subband; work on the
        // bitstream order instead is not possible here, so use band lines).
        let mut first = 576;
        let mut start = 0usize;
        for (b, &w) in short.iter().enumerate() {
            let w = w as usize;
            for win in 0..3 {
                let (a, z) = (start * 3 + win * w, start * 3 + (win + 1) * w);
                if a >= last {
                    let bb = b.min(11);
                    let pos = sf.s[bb][win];
                    let illegal = max.is_some_and(|m| pos == m.s[bb][win]);
                    apply(xr, a, z, pos, illegal);
                    first = first.min(a);
                }
            }
            start += w;
        }
        first
    } else {
        let mut first = 576;
        for b in 0..22 {
            let (a, z) = (long[b] as usize, long[b + 1] as usize);
            if a < last {
                continue;
            }
            let bb = b.min(20);
            let pos = sf.l[bb];
            let illegal = max.is_some_and(|m| pos == m.l[bb]);
            apply(xr, a, z, pos, illegal);
            first = first.min(a);
        }
        first
    }
}

fn antialias(g: &Granule, xr: &mut [f64; 576]) {
    const C: [f64; 8] = [-0.6, -0.535, -0.33, -0.185, -0.095, -0.041, -0.0142, -0.0037];
    let sb_end = if g.switching && g.block_type == 2 { if g.mixed { 1 } else { return } } else { 31 };
    for sb in 0..sb_end {
        for (k, &c) in C.iter().enumerate() {
            let cs = 1.0 / (1.0 + c * c).sqrt();
            let ca = c * cs;
            let (lo, hi) = (sb * 18 + 17 - k, sb * 18 + 18 + k);
            let (a, b) = (xr[lo], xr[hi]);
            xr[lo] = a * cs - b * ca;
            xr[hi] = b * cs + a * ca;
        }
    }
}

struct Imdct {
    long: Vec<f64>,   // [i][k] 36 x 18
    short: Vec<f64>,  // [i][k] 12 x 6
    win: [[f64; 36]; 4],
    win_short: [f64; 12],
}

fn imdct_tables() -> &'static Imdct {
    static T: OnceLock<Imdct> = OnceLock::new();
    T.get_or_init(|| {
        let mut long = vec![0.0; 36 * 18];
        for i in 0..36 {
            for k in 0..18 {
                long[i * 18 + k] = (PI / 72.0 * (2 * i + 1 + 18) as f64 * (2 * k + 1) as f64).cos();
            }
        }
        let mut short = vec![0.0; 12 * 6];
        for i in 0..12 {
            for k in 0..6 {
                short[i * 6 + k] = (PI / 24.0 * (2 * i + 1 + 6) as f64 * (2 * k + 1) as f64).cos();
            }
        }
        let mut win = [[0.0; 36]; 4];
        for i in 0..36 {
            let s36 = (PI / 36.0 * (i as f64 + 0.5)).sin();
            win[0][i] = s36;
            win[1][i] = match i {
                0..=17 => s36,
                18..=23 => 1.0,
                24..=29 => (PI / 12.0 * (i as f64 - 18.0 + 0.5)).sin(),
                _ => 0.0,
            };
            win[3][i] = match i {
                0..=5 => 0.0,
                6..=11 => (PI / 12.0 * (i as f64 - 6.0 + 0.5)).sin(),
                12..=17 => 1.0,
                _ => s36,
            };
        }
        let mut win_short = [0.0; 12];
        for (i, w) in win_short.iter_mut().enumerate() {
            *w = (PI / 12.0 * (i as f64 + 0.5)).sin();
        }
        Imdct { long, short, win, win_short }
    })
}

/// IMDCT + overlap-add + frequency inversion: 576 lines → 18 x 32 subband samples.
fn hybrid(g: &Granule, xr: &[f64; 576], overlap: &mut [f64; 576], out: &mut [[f64; 32]; 18]) {
    let t = imdct_tables();
    for sb in 0..32 {
        let x = &xr[sb * 18..sb * 18 + 18];
        let mut y = [0.0f64; 36];
        let short = g.switching && g.block_type == 2 && !(g.mixed && sb < 2);
        if short {
            for w in 0..3 {
                for i in 0..12 {
                    let mut s = 0.0;
                    for k in 0..6 {
                        s += t.short[i * 6 + k] * x[w * 6 + k];
                    }
                    y[6 + 6 * w + i] += s * t.win_short[i];
                }
            }
        } else if x.iter().any(|&v| v != 0.0) {
            let bt = if g.switching && !(g.mixed && sb < 2) { g.block_type as usize } else { 0 };
            for i in 0..36 {
                let mut s = 0.0;
                for k in 0..18 {
                    s += t.long[i * 18 + k] * x[k];
                }
                y[i] = s * t.win[bt][i];
            }
        }
        for i in 0..18 {
            let mut v = y[i] + overlap[sb * 18 + i];
            overlap[sb * 18 + i] = y[18 + i];
            if sb & 1 == 1 && i & 1 == 1 {
                v = -v;
            }
            out[i][sb] = v;
        }
    }
}

struct Synth {
    v: Vec<f64>,
    off: usize,
}

fn synth_cos() -> &'static Vec<f64> {
    static T: OnceLock<Vec<f64>> = OnceLock::new();
    T.get_or_init(|| {
        let mut n = vec![0.0; 64 * 32];
        for i in 0..64 {
            for k in 0..32 {
                n[i * 32 + k] = ((16 + i) as f64 * (2 * k + 1) as f64 * PI / 64.0).cos();
            }
        }
        n
    })
}

impl Synth {
    fn new() -> Self {
        Synth { v: vec![0.0; 1024], off: 0 }
    }

    /// 32 subband samples → 32 PCM samples.
    fn run(&mut self, s: &[f64; 32], out: &mut Vec<f32>) {
        let n = synth_cos();
        self.off = (self.off + 1024 - 64) % 1024;
        for i in 0..64 {
            let mut acc = 0.0;
            for k in 0..32 {
                acc += n[i * 32 + k] * s[k];
            }
            self.v[(self.off + i) % 1024] = acc;
        }
        for j in 0..32 {
            let mut acc = 0.0;
            for i in 0..8 {
                let u0 = self.v[(self.off + i * 128 + j) % 1024];
                let u1 = self.v[(self.off + i * 128 + 96 + j) % 1024];
                acc += u0 * SYNTH_D[i * 64 + j] as f64 + u1 * SYNTH_D[i * 64 + 32 + j] as f64;
            }
            out.push((acc / 65536.0).clamp(-1.0, 1.0) as f32);
        }
    }
}

/// Skips an ID3v2 tag; returns the offset of the audio.
fn skip_id3(d: &[u8]) -> usize {
    if d.len() >= 10 && &d[..3] == b"ID3" {
        let size = d[6..10].iter().fold(0usize, |a, &b| (a << 7) | (b & 0x7F) as usize);
        let footer = if d[5] & 0x10 != 0 { 10 } else { 0 };
        10 + size + footer
    } else {
        0
    }
}

/// Finds the next frame header at or after `i` whose successor also syncs.
fn find_frame(d: &[u8], mut i: usize) -> Option<(usize, Header)> {
    while i + 4 <= d.len() {
        if let Some(h) = Header::parse(&d[i..]) {
            let next = i + h.frame_len;
            if next + 4 > d.len() || Header::parse(&d[next..]).is_some_and(|n| n.sample_rate == h.sample_rate && n.version == h.version) {
                return Some((i, h));
            }
        }
        i += 1;
    }
    None
}

/// Encoder delay and padding from a Xing/Info frame's LAME tag.
fn xing(frame: &[u8], h: &Header) -> Option<(usize, usize)> {
    let o = 4 + if h.crc { 2 } else { 0 } + h.side_len();
    let tag = frame.get(o..o + 4)?;
    if tag != b"Xing" && tag != b"Info" {
        return None;
    }
    let flags = u32::from_be_bytes(frame.get(o + 4..o + 8)?.try_into().ok()?);
    let mut p = o + 8;
    for (bit, len) in [(1, 4), (2, 4), (4, 100), (8, 4)] {
        if flags & bit != 0 {
            p += len;
        }
    }
    let lame = frame.get(p..p + 24);
    match lame {
        Some(l) if l[..4].iter().all(|c| c.is_ascii_alphanumeric()) => {
            let d = &l[21..24];
            let delay = ((d[0] as usize) << 4) | (d[1] as usize >> 4);
            let pad = ((d[1] as usize & 15) << 8) | d[2] as usize;
            Some((delay, pad))
        }
        _ => Some((0, 0)),
    }
}

/// Decodes a whole MP3 file to interleaved f32 PCM.
pub fn decode(data: &[u8]) -> Result<Wav, String> {
    let start = skip_id3(data);
    let (mut pos, first) = find_frame(data, start).ok_or("no MP3 frames found")?;
    let channels = first.channels();
    let rate = first.sample_rate;
    let mut trim = None;
    if let Some(dp) = xing(&data[pos..(pos + first.frame_len).min(data.len())], &first) {
        trim = Some(dp);
        pos += first.frame_len;
    }
    let mut reservoir: Vec<u8> = Vec::new();
    let mut overlap = [[0.0f64; 576]; 2];
    let mut synth = [Synth::new(), Synth::new()];
    let mut pcm: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    let mut prev_sf = [Scalefactors::default(); 2];
    let mut frames = 0usize;
    while pos + 4 <= data.len() {
        let Some(h) = Header::parse(&data[pos..]) else {
            match find_frame(data, pos + 1) {
                Some((p, _)) => {
                    pos = p;
                    continue;
                }
                None => break,
            }
        };
        if h.sample_rate != rate || h.channels() != channels {
            pos += 1;
            continue;
        }
        let end = (pos + h.frame_len).min(data.len());
        let side_at = pos + 4 + if h.crc { 2 } else { 0 };
        if side_at + h.side_len() > end {
            break;
        }
        let side = read_side(&h, &data[side_at..side_at + h.side_len()]);
        let main = &data[side_at + h.side_len()..end];
        // Bit reservoir: main data starts main_data_begin bytes back.
        let ok = side.main_data_begin <= reservoir.len();
        let keep_from = reservoir.len().saturating_sub(side.main_data_begin);
        let mut buf: Vec<u8> = reservoir[keep_from..].to_vec();
        buf.extend_from_slice(main);
        reservoir.extend_from_slice(main);
        if reservoir.len() > 4096 {
            reservoir.drain(..reservoir.len() - 4096);
        }
        pos = end;
        frames += 1;
        let ngr = h.granules();
        let mut b = Bits::new(&buf);
        for gr in 0..ngr {
            let mut xr = [[0.0f64; 576]; 2];
            let mut nz = [0usize; 2];
            let mut sfs = [Scalefactors::default(); 2];
            let mut max_r: Option<Scalefactors> = None;
            for ch in 0..channels {
                let g = side.gr[gr][ch];
                let part_start = b.pos;
                let (sf, preflag) = if h.version == 0 {
                    (read_scalefactors_v1(&mut b, &g, &side.scfsi[ch], gr, &prev_sf[ch]), g.preflag)
                } else {
                    let is_right = ch == 1 && h.mode == 1 && h.mode_ext & 1 != 0;
                    let (sf, pf, mx) = read_scalefactors_v2(&mut b, &g, is_right);
                    if is_right {
                        max_r = Some(mx);
                    }
                    (sf, pf)
                };
                prev_sf[ch] = sf;
                sfs[ch] = sf;
                let mut is = [0i32; 576];
                nz[ch] = huffman(&mut b, &g, &h, part_start + g.part2_3, &mut is);
                if ok {
                    requantize(&g, &h, &sf, preflag, &is, &mut xr[ch]);
                }
            }
            if channels == 2 {
                stereo(&h, &[side.gr[gr][0], side.gr[gr][1]], &sfs[1], max_r.as_ref(), nz[1], &mut xr);
            }
            for ch in 0..channels {
                let g = side.gr[gr][ch];
                reorder(&g, &h, &mut xr[ch]);
                antialias(&g, &mut xr[ch]);
                let mut sb = [[0.0f64; 32]; 18];
                hybrid(&g, &xr[ch], &mut overlap[ch], &mut sb);
                for s in &sb {
                    synth[ch].run(s, &mut pcm[ch]);
                }
            }
        }
    }
    if frames == 0 {
        return Err("no MP3 frames found".into());
    }
    // Gapless trim: LAME's encoder delay plus the decoder's 529 samples.
    let total = pcm[0].len();
    let (skip, tail) = match trim {
        Some((d, p)) if d > 0 || p > 0 => (d + 529, p.saturating_sub(529)),
        _ => (0, 0),
    };
    let (a, z) = (skip.min(total), total.saturating_sub(tail).max(skip.min(total)));
    let mut samples = Vec::with_capacity((z - a) * channels);
    for i in a..z {
        for c in pcm.iter().take(channels) {
            samples.push(c[i]);
        }
    }
    Ok(Wav { sample_rate: rate, channels: channels as u16, samples })
}

pub fn read(path: &std::path::Path) -> Result<Wav, String> {
    decode(&std::fs::read(path).map_err(|e| e.to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/audio").join(name)).unwrap()
    }

    /// Our decode against ffmpeg's float decode of the same file (`.f32`,
    /// interleaved), allowing a small alignment search.
    fn check(name: &str) {
        let w = decode(&fixture(&format!("{name}.mp3"))).unwrap_or_else(|e| panic!("{name}: {e}"));
        let r: Vec<f32> = fixture(&format!("{name}.f32")).chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let ch = w.channels as usize;
        let n = (r.len() / ch).min(w.samples.len() / ch);
        assert!(n > 1000, "{name}: only {n} frames");
        assert!(((w.samples.len() / ch) as i64 - (r.len() / ch) as i64).abs() <= 1152, "{name}: {} frames vs {}", w.samples.len() / ch, r.len() / ch);
        let mut err = 0.0f64;
        let mut pow = 0.0f64;
        for i in 0..n * ch {
            err += ((w.samples[i] - r[i]) as f64).powi(2);
            pow += (r[i] as f64).powi(2);
        }
        let snr = 10.0 * (pow / err.max(1e-20)).log10();
        eprintln!("{name}: SNR {snr:.1} dB, {n} frames");
        assert!(snr > 60.0, "{name}: SNR {snr:.1} dB");
    }

    #[test]
    fn matches_reference_decodes() {
        for name in ["stereo_128k", "joint_vbr", "mono_22k", "mpeg25_8k", "transient_short"] {
            check(name);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(&[0u8; 3000]).is_err());
    }
}
