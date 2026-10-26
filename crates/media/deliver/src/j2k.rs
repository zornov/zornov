// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A JPEG 2000 (ITU-T T.800 / ISO 15444-1) encoder, written from the
//! standard, for digital cinema and IMF masters.
//!
//! - 9/7 irreversible wavelet, any number of levels, scalar quantisation
//!   with one step per sub-band weighted by its synthesis gain.
//! - EBCOT tier-1: 32 x 32 (or other power-of-two) code-blocks, the three
//!   coding passes, the MQ arithmetic coder.
//! - Rate control by post-compression rate-distortion optimisation: every
//!   code-block is coded once at full depth and cut where the slope of
//!   distortion against bytes crosses a threshold that makes the frame fit
//!   `max_bytes` (the DCI limit is 1 302 083 bytes a frame at 24 fps).
//! - Tier-2: one layer, tag trees, packet headers, precincts, CPRL / RPCL /
//!   LRCP progression, one tile, tile-parts by component with a TLM marker
//!   (what SMPTE ST 429-4 asks of a DCI codestream).
//!
//! Limits: one tile, one quality layer, no ROI, no EPH/SOP, 9/7 only (no
//! lossless 5/3), 1-3 components without sub-sampling. Written with no
//! reference to OpenJPEG or Kakadu source; checked against OpenJPEG's
//! decoder (see the tests) as an independent implementation.
//!
//! Speed: pure Rust on the CPU, split over cores by code-block. A 2K frame
//! is of the order of a second per core: a feature film is a job for the
//! render nodes.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Which order the packets follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progression {
    Lrcp = 0,
    Rlcp = 1,
    Rpcl = 2,
    Pcrl = 3,
    Cprl = 4,
}

#[derive(Clone, Debug)]
pub struct Params {
    pub width: u32,
    pub height: u32,
    pub components: u8,
    /// Bits per sample (12 for cinema).
    pub bit_depth: u8,
    /// Decomposition levels (5 for 2K cinema, 6 for 4K).
    pub levels: u8,
    /// Code-block size exponents (5 = 32).
    pub cb_exp: (u8, u8),
    /// Precinct exponents for resolution 0..=levels (15 = one precinct).
    pub precincts: Vec<u8>,
    pub progression: Progression,
    /// Capabilities word in SIZ: 3 = DCI 2K, 4 = DCI 4K, 0 = none.
    pub rsiz: u16,
    /// Base quantiser step in sample units (smaller = better and bigger).
    pub step: f32,
    /// Cap on the codestream size in bytes.
    pub max_bytes: Option<usize>,
    /// One tile-part per component, and a TLM marker.
    pub tile_parts_by_component: bool,
    pub guard_bits: u8,
    pub threads: usize,
}

impl Params {
    /// The DCI 2K profile (SMPTE ST 429-4): 12 bits, 5 levels, CPRL, 3 tile-parts.
    pub fn dci_2k(width: u32, height: u32, fps: f64) -> Params {
        let mut p = Params::base(width, height);
        p.levels = 5;
        p.precincts = vec![15, 8, 8, 8, 8, 8];
        p.rsiz = 3;
        p.max_bytes = Some(dci_frame_cap(fps));
        p
    }

    /// The DCI 4K profile: 6 levels.
    pub fn dci_4k(width: u32, height: u32, fps: f64) -> Params {
        let mut p = Params::base(width, height);
        p.levels = 6;
        p.precincts = vec![15, 8, 8, 8, 8, 8, 8];
        p.rsiz = 4;
        p.max_bytes = Some(dci_frame_cap(fps));
        p
    }

    /// For a master that is not DCI (IMF): any sizes, one packet per resolution.
    pub fn general(width: u32, height: u32, components: u8, bit_depth: u8) -> Params {
        let mut p = Params::base(width, height);
        p.components = components;
        p.bit_depth = bit_depth;
        p.levels = 5;
        p.precincts = vec![15; 6];
        p.progression = Progression::Cprl;
        p.tile_parts_by_component = false;
        p
    }

    fn base(width: u32, height: u32) -> Params {
        Params { width, height, components: 3, bit_depth: 12, levels: 5, cb_exp: (5, 5), precincts: vec![15; 6], progression: Progression::Cprl, rsiz: 0, step: 1.0, max_bytes: None, tile_parts_by_component: true, guard_bits: 2, threads: std::thread::available_parallelism().map_or(2, |n| n.get()) }
    }
}

/// The DCI limit: 250 Mbit/s in bytes for one frame.
pub fn dci_frame_cap(fps: f64) -> usize {
    (250_000_000.0 / 8.0 / fps.max(1.0)) as usize
}

// ---------------------------------------------------------------- DWT 9/7

const ALPHA: f32 = -1.586_134_3;
const BETA: f32 = -0.052_980_12;
const GAMMA: f32 = 0.882_911_1;
const DELTA: f32 = 0.443_506_85;
const K: f32 = 1.230_174_1;

/// Forward 9/7 on `x` in place; afterwards the low band is `x[..ceil(n/2)]`
/// and the high band follows (deinterleaved).
fn fdwt_1d(x: &mut [f32], tmp: &mut Vec<f32>) {
    let n = x.len();
    if n == 1 {
        return;
    }
    // Whole-sample symmetric extension by index reflection.
    let at = |v: &[f32], i: isize| -> f32 {
        let n = v.len() as isize;
        let mut i = i;
        if i < 0 {
            i = -i;
        }
        if i >= n {
            i = 2 * (n - 1) - i;
        }
        v[i.clamp(0, n - 1) as usize]
    };
    let step = |v: &mut [f32], parity: usize, c: f32| {
        let n = v.len();
        let mut i = parity;
        while i < n {
            let l = at(v, i as isize - 1);
            let r = at(v, i as isize + 1);
            v[i] += c * (l + r);
            i += 2;
        }
    };
    step(x, 1, ALPHA);
    step(x, 0, BETA);
    step(x, 1, GAMMA);
    step(x, 0, DELTA);
    let nl = n.div_ceil(2);
    tmp.clear();
    tmp.resize(n, 0.0);
    for i in 0..n {
        if i % 2 == 0 {
            tmp[i / 2] = x[i] / K;
        } else {
            tmp[nl + i / 2] = x[i] * K;
        }
    }
    x.copy_from_slice(tmp);
}

/// Inverse of [`fdwt_1d`] (used to measure synthesis gains and in tests).
fn idwt_1d(x: &mut [f32], tmp: &mut Vec<f32>) {
    let n = x.len();
    if n == 1 {
        return;
    }
    let nl = n.div_ceil(2);
    tmp.clear();
    tmp.resize(n, 0.0);
    for i in 0..n {
        tmp[i] = if i % 2 == 0 { x[i / 2] * K } else { x[nl + i / 2] / K };
    }
    x.copy_from_slice(tmp);
    let at = |v: &[f32], i: isize| -> f32 {
        let n = v.len() as isize;
        let mut i = i;
        if i < 0 {
            i = -i;
        }
        if i >= n {
            i = 2 * (n - 1) - i;
        }
        v[i.clamp(0, n - 1) as usize]
    };
    let step = |v: &mut [f32], parity: usize, c: f32| {
        let n = v.len();
        let mut i = parity;
        while i < n {
            let l = at(v, i as isize - 1);
            let r = at(v, i as isize + 1);
            v[i] -= c * (l + r);
            i += 2;
        }
    };
    step(x, 0, DELTA);
    step(x, 1, GAMMA);
    step(x, 0, BETA);
    step(x, 1, ALPHA);
}

/// One level of 2-D analysis on the top-left `w x h` of a `stride`-wide buffer.
fn fdwt_2d(buf: &mut [f32], stride: usize, w: usize, h: usize) {
    let mut tmp = Vec::new();
    let mut col = vec![0f32; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = buf[y * stride + x];
        }
        fdwt_1d(&mut col, &mut tmp);
        for y in 0..h {
            buf[y * stride + x] = col[y];
        }
    }
    for y in 0..h {
        fdwt_1d(&mut buf[y * stride..y * stride + w], &mut tmp);
    }
}

fn idwt_2d(buf: &mut [f32], stride: usize, w: usize, h: usize) {
    let mut tmp = Vec::new();
    for y in 0..h {
        idwt_1d(&mut buf[y * stride..y * stride + w], &mut tmp);
    }
    let mut col = vec![0f32; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = buf[y * stride + x];
        }
        idwt_1d(&mut col, &mut tmp);
        for y in 0..h {
            buf[y * stride + x] = col[y];
        }
    }
}

/// Full forward transform: `levels` of analysis in Mallat layout.
fn fdwt(buf: &mut [f32], w: usize, h: usize, levels: usize) {
    let (mut cw, mut ch) = (w, h);
    for _ in 0..levels {
        fdwt_2d(buf, w, cw, ch);
        cw = cw.div_ceil(2);
        ch = ch.div_ceil(2);
    }
}

// ------------------------------------------------------------- sub-bands

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Orient {
    Ll,
    Hl,
    Lh,
    Hh,
}

#[derive(Clone, Debug)]
struct Band {
    res: usize,
    orient: Orient,
    /// Position and size in the Mallat buffer.
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    /// Quantiser: exponent, mantissa, resulting step.
    eps: u32,
    mu: u32,
    delta: f32,
    /// Bit-planes the band can hold (guard + eps - 1).
    mb: u32,
    /// Squared synthesis gain times step squared: image-domain error per unit squared quantised error.
    dist_w: f64,
}

/// The sub-bands of an image `w x h` after `levels`, in packet order
/// (resolution 0: LL; then HL, LH, HH of each resolution).
fn bands_of(w: usize, h: usize, levels: usize) -> Vec<Band> {
    let mut sizes = vec![(w, h)];
    for _ in 0..levels {
        let (pw, ph) = *sizes.last().unwrap();
        sizes.push((pw.div_ceil(2), ph.div_ceil(2)));
    }
    let mut out = Vec::new();
    let (lw, lh) = sizes[levels];
    out.push(Band { res: 0, orient: Orient::Ll, x0: 0, y0: 0, w: lw, h: lh, eps: 0, mu: 0, delta: 1.0, mb: 0, dist_w: 1.0 });
    for r in 1..=levels {
        // The image at the finer stage of this resolution.
        let (fw, fh) = sizes[levels - r];
        let (cw, ch) = sizes[levels - r + 1];
        let (hw, hh) = (fw - cw, fh - ch);
        out.push(Band { res: r, orient: Orient::Hl, x0: cw, y0: 0, w: hw, h: ch, eps: 0, mu: 0, delta: 1.0, mb: 0, dist_w: 1.0 });
        out.push(Band { res: r, orient: Orient::Lh, x0: 0, y0: ch, w: cw, h: hh, eps: 0, mu: 0, delta: 1.0, mb: 0, dist_w: 1.0 });
        out.push(Band { res: r, orient: Orient::Hh, x0: cw, y0: ch, w: hw, h: hh, eps: 0, mu: 0, delta: 1.0, mb: 0, dist_w: 1.0 });
    }
    out
}

/// L2 norm of the synthesis basis function of each band, measured by
/// inverting a single coefficient in a large enough image.
fn synthesis_norms(levels: usize) -> Vec<f64> {
    let side = (1usize << levels) * 24;
    let bands = bands_of(side, side, levels);
    bands
        .iter()
        .map(|b| {
            let mut buf = vec![0f32; side * side];
            buf[(b.y0 + b.h / 2) * side + b.x0 + b.w / 2] = 1.0;
            // Inverse: coarsest first.
            for l in (0..levels).rev() {
                let (mut cw, mut ch) = (side, side);
                for _ in 0..l {
                    cw = cw.div_ceil(2);
                    ch = ch.div_ceil(2);
                }
                idwt_2d(&mut buf, side, cw, ch);
            }
            buf.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>().sqrt()
        })
        .collect()
}

fn log2_gain(o: Orient) -> u32 {
    match o {
        Orient::Ll => 0,
        Orient::Hl | Orient::Lh => 1,
        Orient::Hh => 2,
    }
}

// ------------------------------------------------------------- MQ coder

const QE: [(u16, u8, u8, u8); 47] = [
    (0x5601, 1, 1, 1),
    (0x3401, 2, 6, 0),
    (0x1801, 3, 9, 0),
    (0x0AC1, 4, 12, 0),
    (0x0521, 5, 29, 0),
    (0x0221, 38, 33, 0),
    (0x5601, 7, 6, 1),
    (0x5401, 8, 14, 0),
    (0x4801, 9, 14, 0),
    (0x3801, 10, 14, 0),
    (0x3001, 11, 17, 0),
    (0x2401, 12, 18, 0),
    (0x1C01, 13, 20, 0),
    (0x1601, 29, 21, 0),
    (0x5601, 15, 14, 1),
    (0x5401, 16, 14, 0),
    (0x5101, 17, 15, 0),
    (0x4801, 18, 16, 0),
    (0x3801, 19, 17, 0),
    (0x3401, 20, 18, 0),
    (0x3001, 21, 19, 0),
    (0x2801, 22, 19, 0),
    (0x2401, 23, 20, 0),
    (0x2201, 24, 21, 0),
    (0x1C01, 25, 22, 0),
    (0x1801, 26, 23, 0),
    (0x1601, 27, 24, 0),
    (0x1401, 28, 25, 0),
    (0x1201, 29, 26, 0),
    (0x1101, 30, 27, 0),
    (0x0AC1, 31, 28, 0),
    (0x09C1, 32, 29, 0),
    (0x08A1, 33, 30, 0),
    (0x0521, 34, 31, 0),
    (0x0441, 35, 32, 0),
    (0x02A1, 36, 33, 0),
    (0x0221, 37, 34, 0),
    (0x0141, 38, 35, 0),
    (0x0111, 39, 36, 0),
    (0x0085, 40, 37, 0),
    (0x0049, 41, 38, 0),
    (0x0025, 42, 39, 0),
    (0x0015, 43, 40, 0),
    (0x0009, 44, 41, 0),
    (0x0005, 45, 42, 0),
    (0x0001, 45, 43, 0),
    (0x5601, 46, 46, 0),
];

const CTX_RUN: usize = 17;
const CTX_UNI: usize = 18;

struct Mq {
    a: u32,
    c: u32,
    ct: u32,
    /// out[0] is a placeholder; `bp` is the index of the last byte written.
    out: Vec<u8>,
    bp: usize,
    state: [(u8, u8); 19],
}

impl Mq {
    fn new() -> Mq {
        let mut state = [(0u8, 0u8); 19];
        state[0] = (4, 0);
        state[CTX_RUN] = (3, 0);
        state[CTX_UNI] = (46, 0);
        Mq { a: 0x8000, c: 0, ct: 12, out: vec![0u8], bp: 0, state }
    }

    fn bytes(&self) -> usize {
        self.bp
    }

    fn byteout(&mut self) {
        if self.out[self.bp] == 0xff {
            self.bp += 1;
            self.push_at((self.c >> 20) as u8);
            self.c &= 0xfffff;
            self.ct = 7;
        } else if self.c & 0x800_0000 == 0 {
            self.bp += 1;
            self.push_at((self.c >> 19) as u8);
            self.c &= 0x7ffff;
            self.ct = 8;
        } else {
            self.out[self.bp] += 1;
            if self.out[self.bp] == 0xff {
                self.c &= 0x7ff_ffff;
                self.bp += 1;
                self.push_at((self.c >> 20) as u8);
                self.c &= 0xfffff;
                self.ct = 7;
            } else {
                self.bp += 1;
                self.push_at((self.c >> 19) as u8);
                self.c &= 0x7ffff;
                self.ct = 8;
            }
        }
    }

    fn push_at(&mut self, b: u8) {
        if self.bp < self.out.len() {
            self.out[self.bp] = b;
        } else {
            self.out.push(b);
        }
    }

    fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byteout();
            }
            if self.a & 0x8000 != 0 {
                break;
            }
        }
    }

    fn encode(&mut self, ctx: usize, d: u8) {
        let (idx, mps) = self.state[ctx];
        let (qe, nmps, nlps, sw) = QE[idx as usize];
        let qe = qe as u32;
        if d == mps {
            self.a -= qe;
            if self.a & 0x8000 == 0 {
                if self.a < qe {
                    self.a = qe;
                } else {
                    self.c += qe;
                }
                self.state[ctx].0 = nmps;
                self.renorm();
            } else {
                self.c += qe;
            }
        } else {
            self.a -= qe;
            if self.a < qe {
                self.c += qe;
            } else {
                self.a = qe;
            }
            if sw == 1 {
                self.state[ctx].1 = 1 - mps;
            }
            self.state[ctx].0 = nlps;
            self.renorm();
        }
    }

    /// Terminate; the codeword is `out[1..=bp]` (without a trailing 0xFF).
    fn flush(&mut self) {
        let temp = self.c + self.a;
        self.c |= 0xffff;
        if self.c >= temp {
            self.c -= 0x8000;
        }
        self.c <<= self.ct;
        self.byteout();
        self.c <<= self.ct;
        self.byteout();
        if self.out[self.bp] == 0xff {
            self.bp -= 1;
        }
    }
}

// ----------------------------------------------------- tier-1 block coder

#[derive(Clone, Debug)]
struct Pass {
    /// Bytes of the codeword needed to decode through this pass.
    rate: usize,
    /// Distortion removed by this pass (image-domain squared error).
    dist: f64,
}

#[derive(Clone, Debug, Default)]
struct CodedBlock {
    data: Vec<u8>,
    passes: Vec<Pass>,
    /// All-zero bit-planes at the top (Mb - coded planes).
    zero_planes: u32,
}

const F_SIG: u8 = 1;
const F_VISIT: u8 = 2;
const F_REFINED: u8 = 4;
const F_NEG: u8 = 8;

struct Block {
    w: usize,
    /// Flags with a one-sample border.
    f: Vec<u8>,
    /// Which of the eight neighbours are significant: left, right, up, down, up-left, up-right, down-left, down-right.
    nb: Vec<u8>,
    /// Whether the four direct neighbours that are significant are negative (same order, low four bits).
    sg: Vec<u8>,
    orient: Orient,
}

const N_L: u8 = 1;
const N_R: u8 = 2;
const N_U: u8 = 4;
const N_D: u8 = 8;

/// Zero-coding context for each neighbour pattern, per orientation (LL/LH, HL, HH).
fn zc_lut() -> &'static [[u8; 256]; 3] {
    static L: std::sync::OnceLock<[[u8; 256]; 3]> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut t = [[0u8; 256]; 3];
        for (o, row) in t.iter_mut().enumerate() {
            for (n, cell) in row.iter_mut().enumerate() {
                let (mut h, mut v) = (((n & 1) + ((n >> 1) & 1)) as u32, (((n >> 2) & 1) + ((n >> 3) & 1)) as u32);
                let d = ((n >> 4) & 15).count_ones();
                *cell = if o == 2 {
                    let hv = h + v;
                    match d {
                        0 => hv.min(2) as u8,
                        1 => 3 + hv.min(2) as u8,
                        2 => 6 + hv.min(1) as u8,
                        _ => 8,
                    }
                } else {
                    if o == 1 {
                        std::mem::swap(&mut h, &mut v);
                    }
                    match h {
                        2 => 8,
                        1 => {
                            if v >= 1 {
                                7
                            } else if d >= 1 {
                                6
                            } else {
                                5
                            }
                        }
                        _ => match v {
                            2 => 4,
                            1 => 3,
                            _ => d.min(2) as u8,
                        },
                    }
                };
            }
        }
        t
    })
}

/// Sign-coding context and xor bit from (direct neighbours significant, their signs): index `nb & 15 | (sg & 15) << 4`.
fn sc_lut() -> &'static [(u8, u8); 256] {
    static L: std::sync::OnceLock<[(u8, u8); 256]> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut t = [(9u8, 0u8); 256];
        for (i, cell) in t.iter_mut().enumerate() {
            let (nb, sg) = (i & 15, i >> 4);
            let contrib = |bit: usize| -> i32 {
                if nb & (1 << bit) == 0 {
                    0
                } else if sg & (1 << bit) != 0 {
                    -1
                } else {
                    1
                }
            };
            let h = (contrib(0) + contrib(1)).clamp(-1, 1);
            let v = (contrib(2) + contrib(3)).clamp(-1, 1);
            *cell = match (h, v) {
                (1, 1) => (13, 0),
                (1, 0) => (12, 0),
                (1, -1) => (11, 0),
                (0, 1) => (10, 0),
                (0, 0) => (9, 0),
                (0, -1) => (10, 1),
                (-1, 1) => (11, 1),
                (-1, 0) => (12, 1),
                _ => (13, 1),
            };
        }
        t
    })
}

impl Block {
    fn idx(&self, x: usize, y: usize) -> usize {
        (y + 1) * (self.w + 2) + x + 1
    }

    #[inline]
    fn zc_ctx(&self, i: usize) -> usize {
        zc_lut()[match self.orient {
            Orient::Hl => 1,
            Orient::Hh => 2,
            _ => 0,
        }][self.nb[i] as usize] as usize
    }

    /// Sign-coding context and the bit to xor with the sign.
    #[inline]
    fn sc(&self, i: usize) -> (usize, u8) {
        let (c, x) = sc_lut()[((self.nb[i] & 15) | ((self.sg[i] & 15) << 4)) as usize];
        (c as usize, x)
    }

    #[inline]
    fn mr_ctx(&self, i: usize) -> usize {
        if self.f[i] & F_REFINED != 0 {
            16
        } else if self.nb[i] != 0 {
            15
        } else {
            14
        }
    }

    /// Marks `i` significant and tells its eight neighbours.
    fn set_sig(&mut self, i: usize, neg: bool) {
        let s = self.w + 2;
        self.f[i] |= F_SIG | if neg { F_NEG } else { 0 };
        let n = neg as u8;
        self.nb[i - 1] |= N_R;
        self.sg[i - 1] |= n << 1;
        self.nb[i + 1] |= N_L;
        self.sg[i + 1] |= n;
        self.nb[i - s] |= N_D;
        self.sg[i - s] |= n << 3;
        self.nb[i + s] |= N_U;
        self.sg[i + s] |= n << 2;
        self.nb[i - s - 1] |= 1 << 7;
        self.nb[i - s + 1] |= 1 << 6;
        self.nb[i + s - 1] |= 1 << 5;
        self.nb[i + s + 1] |= 1 << 4;
    }
}

/// Code one block of quantised magnitudes. `dist_w` scales distortion.
fn code_block(w: usize, h: usize, mag: &[u32], neg: &[bool], orient: Orient, mb: u32, dist_w: f64) -> CodedBlock {
    let maxv = mag.iter().copied().max().unwrap_or(0);
    if maxv == 0 {
        return CodedBlock::default();
    }
    let nbps = 32 - maxv.leading_zeros();
    let cells = (w + 2) * (h + 2);
    let mut b = Block { w, f: vec![0u8; cells], nb: vec![0u8; cells], sg: vec![0u8; cells], orient };
    let mut mq = Mq::new();
    let mut passes: Vec<Pass> = Vec::new();
    // Reconstruction in quantised units, for the distortion of each pass.
    let mut recon = vec![0f32; w * h];
    let dist_of = |before: f32, after: f32, v: f32| -> f64 { (((v - before) * (v - before)) - ((v - after) * (v - after))) as f64 };
    let mut pass_dist = 0f64;

    for bp in (0..nbps).rev() {
        let bit = |x: usize, y: usize| (mag[y * w + x] >> bp) & 1;
        // 1: significance propagation, 2: refinement (not for the first plane), 3: cleanup.
        if bp != nbps - 1 {
            // Significance propagation.
            for k in (0..h).step_by(4) {
                for x in 0..w {
                    for y in k..(k + 4).min(h) {
                        let i = b.idx(x, y);
                        if b.f[i] & F_SIG != 0 {
                            continue;
                        }
                        let ctx = b.zc_ctx(i);
                        if ctx == 0 {
                            continue;
                        }
                        let d = bit(x, y) as u8;
                        mq.encode(ctx, d);
                        b.f[i] |= F_VISIT;
                        if d == 1 {
                            let (sc, xor) = b.sc(i);
                            mq.encode(sc, neg[y * w + x] as u8 ^ xor);
                            b.set_sig(i, neg[y * w + x]);
                            let v = mag[y * w + x] as f32;
                            let r = 1.5 * (1u32 << bp) as f32;
                            pass_dist += dist_of(0.0, r, v);
                            recon[y * w + x] = r;
                        }
                    }
                }
            }
            passes.push(Pass { rate: mq.bytes(), dist: pass_dist * dist_w });
            pass_dist = 0.0;
            // Magnitude refinement.
            for k in (0..h).step_by(4) {
                for x in 0..w {
                    for y in k..(k + 4).min(h) {
                        let i = b.idx(x, y);
                        if b.f[i] & F_SIG == 0 || b.f[i] & F_VISIT != 0 {
                            continue;
                        }
                        let ctx = b.mr_ctx(i);
                        mq.encode(ctx, bit(x, y) as u8);
                        b.f[i] |= F_REFINED;
                        let v = mag[y * w + x] as f32;
                        let m = ((mag[y * w + x] >> bp) << bp) as f32;
                        let r = if bp == 0 { v } else { m + (1u32 << (bp - 1)) as f32 };
                        pass_dist += dist_of(recon[y * w + x], r, v);
                        recon[y * w + x] = r;
                    }
                }
            }
            passes.push(Pass { rate: mq.bytes(), dist: pass_dist * dist_w });
            pass_dist = 0.0;
        }
        // Cleanup.
        for k in (0..h).step_by(4) {
            for x in 0..w {
                let mut y = k;
                let full = k + 4 <= h;
                let mut run_done = false;
                if full {
                    let eligible = (0..4).all(|r| {
                        let i = b.idx(x, k + r);
                        b.f[i] & (F_SIG | F_VISIT) == 0 && b.zc_ctx(i) == 0
                    });
                    if eligible {
                        let first = (0..4).find(|&r| bit(x, k + r) == 1);
                        match first {
                            None => {
                                mq.encode(CTX_RUN, 0);
                                y = k + 4;
                            }
                            Some(r) => {
                                mq.encode(CTX_RUN, 1);
                                mq.encode(CTX_UNI, ((r >> 1) & 1) as u8);
                                mq.encode(CTX_UNI, (r & 1) as u8);
                                let yy = k + r;
                                let i = b.idx(x, yy);
                                let (sc, xor) = b.sc(i);
                                mq.encode(sc, neg[yy * w + x] as u8 ^ xor);
                                b.set_sig(i, neg[yy * w + x]);
                                let v = mag[yy * w + x] as f32;
                                let rr = 1.5 * (1u32 << bp) as f32;
                                pass_dist += dist_of(0.0, rr, v);
                                recon[yy * w + x] = rr;
                                y = yy + 1;
                            }
                        }
                        run_done = true;
                    }
                }
                let _ = run_done;
                while y < (k + 4).min(h) {
                    let i = b.idx(x, y);
                    if b.f[i] & (F_SIG | F_VISIT) == 0 {
                        let ctx = b.zc_ctx(i);
                        let d = bit(x, y) as u8;
                        mq.encode(ctx, d);
                        if d == 1 {
                            let (sc, xor) = b.sc(i);
                            mq.encode(sc, neg[y * w + x] as u8 ^ xor);
                            b.set_sig(i, neg[y * w + x]);
                            let v = mag[y * w + x] as f32;
                            let rr = 1.5 * (1u32 << bp) as f32;
                            pass_dist += dist_of(0.0, rr, v);
                            recon[y * w + x] = rr;
                        }
                    }
                    y += 1;
                }
            }
        }
        // Visit flags last one plane.
        for f in b.f.iter_mut() {
            *f &= !F_VISIT;
        }
        passes.push(Pass { rate: mq.bytes(), dist: pass_dist * dist_w });
        pass_dist = 0.0;
    }
    mq.flush();
    let total = mq.bp;
    // Rates so far are the bytes already out; a pass needs a few more to be decodable.
    for p in &mut passes {
        p.rate = (p.rate + 4).min(total);
    }
    if let Some(last) = passes.last_mut() {
        last.rate = total;
    }
    // Rates must not decrease.
    let mut prev = 0;
    for p in &mut passes {
        p.rate = p.rate.max(prev);
        prev = p.rate;
    }
    CodedBlock { data: mq.out[1..=total].to_vec(), passes, zero_planes: mb.saturating_sub(nbps) }
}

// ----------------------------------------------------------------- tier-2

struct BitWriter {
    out: Vec<u8>,
    cur: u8,
    n: u32,
    /// The byte before was 0xFF: this one holds 7 bits.
    stuff: bool,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { out: Vec::new(), cur: 0, n: 0, stuff: false }
    }

    fn put(&mut self, bit: u32) {
        let cap = if self.stuff { 7 } else { 8 };
        self.cur = (self.cur << 1) | (bit & 1) as u8;
        self.n += 1;
        if self.n == cap {
            self.out.push(self.cur);
            self.stuff = self.cur == 0xff;
            self.cur = 0;
            self.n = 0;
        }
    }

    fn put_bits(&mut self, v: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.put((v >> i) & 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let cap = if self.stuff { 7 } else { 8 };
            self.cur <<= cap - self.n;
            self.out.push(self.cur);
            self.stuff = self.cur == 0xff;
        }
        if self.stuff {
            self.out.push(0);
        }
        self.out
    }
}

struct TagTree {
    /// Per level (leaves first): width, height, node values / lows / known.
    levels: Vec<(usize, usize)>,
    value: Vec<Vec<u32>>,
    low: Vec<Vec<u32>>,
    known: Vec<Vec<bool>>,
}

impl TagTree {
    fn new(w: usize, h: usize) -> TagTree {
        let mut levels = vec![(w.max(1), h.max(1))];
        while levels.last().map(|l| l.0 * l.1).unwrap_or(1) > 1 {
            let (lw, lh) = *levels.last().unwrap();
            levels.push((lw.div_ceil(2), lh.div_ceil(2)));
        }
        let value = levels.iter().map(|l| vec![u32::MAX; l.0 * l.1]).collect();
        let low = levels.iter().map(|l| vec![0u32; l.0 * l.1]).collect();
        let known = levels.iter().map(|l| vec![false; l.0 * l.1]).collect();
        TagTree { levels, value, low, known }
    }

    fn set(&mut self, x: usize, y: usize, v: u32) {
        let (mut x, mut y) = (x, y);
        for l in 0..self.levels.len() {
            let i = y * self.levels[l].0 + x;
            if v < self.value[l][i] {
                self.value[l][i] = v;
            } else {
                break;
            }
            x /= 2;
            y /= 2;
        }
    }

    fn encode(&mut self, bw: &mut BitWriter, x: usize, y: usize, threshold: u32) {
        let top = self.levels.len() - 1;
        let mut low = 0u32;
        for l in (0..=top).rev() {
            let i = (y >> l) * self.levels[l].0 + (x >> l);
            if low > self.low[l][i] {
                self.low[l][i] = low;
            } else {
                low = self.low[l][i];
            }
            while low < threshold {
                if low >= self.value[l][i] {
                    if !self.known[l][i] {
                        bw.put(1);
                        self.known[l][i] = true;
                    }
                    break;
                }
                bw.put(0);
                low += 1;
            }
            self.low[l][i] = low;
        }
    }
}

fn put_num_passes(bw: &mut BitWriter, n: u32) {
    match n {
        1 => bw.put(0),
        2 => bw.put_bits(0b10, 2),
        3..=5 => {
            bw.put_bits(0b11, 2);
            bw.put_bits(n - 3, 2);
        }
        6..=36 => {
            bw.put_bits(0b1111, 4);
            bw.put_bits(n - 6, 5);
        }
        _ => {
            bw.put_bits(0x1ff, 9);
            bw.put_bits(n - 37, 7);
        }
    }
}

fn floor_log2(v: u32) -> u32 {
    31 - v.max(1).leading_zeros()
}

/// A code-block placed in the packet structure.
struct PlacedBlock {
    /// Position among the blocks of its precinct-band (for the tag trees).
    bx: usize,
    by: usize,
    /// Index into the component's block list.
    block: usize,
}

struct PrecinctBand {
    blocks_w: usize,
    blocks_h: usize,
    placed: Vec<PlacedBlock>,
}

/// One precinct of one resolution of a component.
struct Precinct {
    res: usize,
    /// Position of its top-left corner in the reference grid.
    x: usize,
    y: usize,
    bands: Vec<PrecinctBand>,
}

// --------------------------------------------------------------- encoder

struct CompBlocks {
    /// All code-blocks of the component in band order, with their band and grid position.
    blocks: Vec<(usize, CodedBlock)>,
    precincts: Vec<Precinct>,
}

fn ceil_div(a: usize, b: usize) -> usize {
    a.div_ceil(b)
}

/// Encode samples to a codestream. `planes[c]` holds `width * height`
/// unsigned samples of `bit_depth` bits.
pub fn encode(planes: &[&[i32]], p: &Params) -> Result<Vec<u8>, String> {
    let (w, h) = (p.width as usize, p.height as usize);
    let nc = p.components as usize;
    if planes.len() != nc || nc == 0 || nc > 4 {
        return Err("one plane per component, 1-4 components".into());
    }
    if planes.iter().any(|pl| pl.len() != w * h) {
        return Err("plane size does not match width x height".into());
    }
    if w == 0 || h == 0 || p.levels as usize > 10 || p.precincts.len() != p.levels as usize + 1 {
        return Err("bad parameters".into());
    }
    let nl = p.levels as usize;
    let norms = synthesis_norms(nl);
    // Quantiser per band (shared by all components).
    let mut template = bands_of(w, h, nl);
    for (bi, b) in template.iter_mut().enumerate() {
        let rb = p.bit_depth as u32 + log2_gain(b.orient);
        let delta = (p.step as f64 / norms[bi]).max(1e-6);
        let e = delta.log2().floor() as i32;
        let eps = (rb as i32 - e).clamp(0, 31) as u32;
        let base = 2f64.powi(rb as i32 - eps as i32);
        let mu = (((delta / base) - 1.0) * 2048.0).round().clamp(0.0, 2047.0) as u32;
        let step = (base * (1.0 + mu as f64 / 2048.0)) as f32;
        b.eps = eps;
        b.mu = mu;
        b.delta = step;
        b.mb = (p.guard_bits as u32 + eps).saturating_sub(1);
        // Image-domain squared error per unit squared quantised error.
        b.dist_w = (norms[bi] * step as f64).powi(2);
    }

    // Transform and code every component.
    let mut comps: Vec<CompBlocks> = Vec::new();
    let next = AtomicUsize::new(0);
    for plane in planes {
        let mut buf: Vec<f32> = plane.iter().map(|&v| (v - (1 << (p.bit_depth - 1))) as f32).collect();
        fdwt(&mut buf, w, h, nl);
        // Quantise into sign + magnitude per band, then split into blocks.
        let (xcb, ycb) = (p.cb_exp.0 as usize, p.cb_exp.1 as usize);
        struct Job {
            band: usize,
            w: usize,
            h: usize,
            mag: Vec<u32>,
            neg: Vec<bool>,
        }
        let mut jobs: Vec<Job> = Vec::new();
        let mut grid: Vec<(usize, usize, usize)> = Vec::new(); // (band, bx, by) per job
        for (bi, b) in template.iter().enumerate() {
            if b.w == 0 || b.h == 0 {
                continue;
            }
            // Code-block size limited by the precinct in this band.
            let pp = p.precincts[b.res] as usize;
            let pband = if b.res == 0 { pp } else { pp.saturating_sub(1) };
            let (cw, ch) = (1usize << xcb.min(pband), 1usize << ycb.min(pband));
            for by in 0..ceil_div(b.h, ch) {
                for bx in 0..ceil_div(b.w, cw) {
                    let (x0, y0) = (bx * cw, by * ch);
                    let (bw, bh) = (cw.min(b.w - x0), ch.min(b.h - y0));
                    let mut mag = Vec::with_capacity(bw * bh);
                    let mut neg = Vec::with_capacity(bw * bh);
                    for y in 0..bh {
                        for x in 0..bw {
                            let c = buf[(b.y0 + y0 + y) * w + b.x0 + x0 + x];
                            let q = (c.abs() / b.delta).floor() as u32;
                            mag.push(q.min((1 << b.mb) - 1));
                            neg.push(c < 0.0 && q != 0);
                        }
                    }
                    jobs.push(Job { band: bi, w: bw, h: bh, mag, neg });
                    grid.push((bi, bx, by));
                }
            }
        }
        let results: Vec<std::sync::Mutex<Option<CodedBlock>>> = jobs.iter().map(|_| std::sync::Mutex::new(None)).collect();
        next.store(0, Ordering::Relaxed);
        std::thread::scope(|s| {
            for _ in 0..p.threads.max(1) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= jobs.len() {
                            break;
                        }
                        let j = &jobs[i];
                        let b = &template[j.band];
                        let cb = code_block(j.w, j.h, &j.mag, &j.neg, b.orient, b.mb, b.dist_w);
                        *results[i].lock().unwrap() = Some(cb);
                    }
                });
            }
        });
        let blocks: Vec<(usize, CodedBlock)> = results.into_iter().zip(&grid).map(|(r, g)| (g.0, r.into_inner().unwrap().unwrap_or_default())).collect();
        // Precincts: group blocks.
        let precincts = build_precincts(&template, &grid, p, w, h);
        comps.push(CompBlocks { blocks, precincts });
    }

    // Packet order over the whole stream.
    let order = packet_order(p, &comps);
    let assemble = |keeps: &[Vec<usize>]| -> Vec<Vec<u8>> {
        let mut parts: Vec<Vec<u8>> = Vec::new();
        let mut last_comp = usize::MAX;
        for &(ci, pi) in &order {
            if parts.is_empty() || (p.tile_parts_by_component && ci != last_comp) {
                parts.push(Vec::new());
            }
            last_comp = ci;
            let pk = packet(&comps[ci].precincts[pi], &comps[ci], &keeps[ci]);
            parts.last_mut().unwrap().extend(pk);
        }
        parts
    };
    let header_len = main_header(p, &template).len();
    let total = |parts: &Vec<Vec<u8>>| header_len + parts.iter().map(|x| x.len() + 14).sum::<usize>() + 12 + 6 * parts.len() + 2;
    let keep_all: Vec<Vec<usize>> = comps.iter().map(|c| c.blocks.iter().map(|(_, cb)| cb.passes.len()).collect()).collect();
    let mut tile_parts = assemble(&keep_all);
    if let Some(cap) = p.max_bytes {
        if total(&tile_parts) > cap {
            // Slope threshold search: each block keeps the passes on its
            // convex hull whose slope is at least lambda.
            let hulls: Vec<Vec<Vec<(usize, f64)>>> = comps.iter().map(|c| c.blocks.iter().map(|(_, cb)| convex_hull(cb)).collect()).collect();
            let keeps_at = |lam: f64| -> Vec<Vec<usize>> { hulls.iter().map(|hc| hc.iter().map(|hb| hull_keep(hb, lam)).collect()).collect() };
            // Slopes span many orders of magnitude: bisect on the logarithm.
            let (mut lo, mut hi) = (-12f64, 20f64);
            let mut best: Option<Vec<Vec<u8>>> = None;
            for _ in 0..40 {
                let mid = (lo + hi) / 2.0;
                let parts = assemble(&keeps_at(10f64.powf(mid)));
                if total(&parts) <= cap {
                    hi = mid;
                    best = Some(parts);
                } else {
                    lo = mid;
                }
                if hi - lo < 0.002 {
                    break;
                }
            }
            if best.is_none() {
                let parts = assemble(&keeps_at(10f64.powf(hi)));
                if total(&parts) <= cap {
                    best = Some(parts);
                }
            }
            tile_parts = best.ok_or("the picture cannot be coded within the byte limit")?;
        }
    }

    // Codestream.
    let mut out = main_header(p, &template);
    if p.tile_parts_by_component {
        // TLM: Ltlm, Ztlm, Stlm = 0x60 (16-bit tile index, 32-bit length), one entry per tile-part.
        let n = tile_parts.len();
        out.extend_from_slice(&[0xff, 0x55]);
        out.extend_from_slice(&((4 + 6 * n) as u16).to_be_bytes());
        out.push(0);
        out.push(0x60);
        for tp in &tile_parts {
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&((tp.len() + 14) as u32).to_be_bytes());
        }
    }
    let nparts = tile_parts.len();
    for (i, tp) in tile_parts.iter().enumerate() {
        out.extend_from_slice(&[0xff, 0x90]);
        out.extend_from_slice(&10u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&((tp.len() + 14) as u32).to_be_bytes());
        out.push(i as u8);
        out.push(nparts as u8);
        out.extend_from_slice(&[0xff, 0x93]);
        out.extend_from_slice(tp);
    }
    out.extend_from_slice(&[0xff, 0xd9]);
    Ok(out)
}

/// The order packets follow in the stream: (component, precinct index).
fn packet_order(p: &Params, comps: &[CompBlocks]) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize)> = Vec::new();
    for (ci, c) in comps.iter().enumerate() {
        for pi in 0..c.precincts.len() {
            all.push((ci, pi));
        }
    }
    let pr = |&(ci, pi): &(usize, usize)| &comps[ci].precincts[pi];
    match p.progression {
        // One layer: layer is constant.
        Progression::Lrcp | Progression::Rlcp => all.sort_by_key(|k| (pr(k).res, k.0, k.1)),
        Progression::Rpcl => all.sort_by_key(|k| (pr(k).res, pr(k).y, pr(k).x, k.0)),
        Progression::Pcrl => all.sort_by_key(|k| (pr(k).y, pr(k).x, k.0, pr(k).res)),
        Progression::Cprl => all.sort_by_key(|k| (k.0, pr(k).y, pr(k).x, pr(k).res)),
    }
    all
}

/// The convex hull of (rate, distortion reduction): the truncation points
/// worth considering, as (passes kept, slope to reach it from the last).
fn convex_hull(cb: &CodedBlock) -> Vec<(usize, f64)> {
    // Points (rate, cumulative distortion reduction).
    let mut pts: Vec<(usize, f64, f64)> = Vec::new(); // (passes, rate, dist)
    let (mut cum, mut _r) = (0f64, 0usize);
    for (i, p) in cb.passes.iter().enumerate() {
        cum += p.dist;
        _r = p.rate;
        pts.push((i + 1, p.rate as f64, cum));
    }
    let mut hull: Vec<(usize, f64, f64)> = Vec::new();
    for pt in pts {
        while let Some(&last) = hull.last() {
            let (lr, ld) = (last.1, last.2);
            let prev = if hull.len() >= 2 { let q = hull[hull.len() - 2]; (q.1, q.2) } else { (0.0, 0.0) };
            let s_last = (ld - prev.1) / (lr - prev.0).max(1e-9);
            let s_new = (pt.2 - ld) / (pt.1 - lr).max(1e-9);
            if s_new >= s_last || pt.1 <= lr {
                hull.pop();
            } else {
                break;
            }
        }
        hull.push(pt);
    }
    let mut out = Vec::new();
    let (mut pr, mut pd) = (0.0, 0.0);
    for (n, r, d) in hull {
        out.push((n, (d - pd) / (r - pr).max(1e-9)));
        pr = r;
        pd = d;
    }
    out
}

fn hull_keep(hull: &[(usize, f64)], lam: f64) -> usize {
    let mut n = 0;
    for &(passes, slope) in hull {
        if slope >= lam {
            n = passes;
        } else {
            break;
        }
    }
    n
}

fn build_precincts(bands: &[Band], grid: &[(usize, usize, usize)], p: &Params, w: usize, h: usize) -> Vec<Precinct> {
    let nl = p.levels as usize;
    let lookup: std::collections::HashMap<(usize, usize, usize), usize> = grid.iter().enumerate().map(|(i, g)| (*g, i)).collect();
    let mut out = Vec::new();
    for r in 0..=nl {
        let sh = nl - r;
        let (rw, rh) = (ceil_div(w, 1 << sh), ceil_div(h, 1 << sh));
        let pp = p.precincts[r] as usize;
        let (npw, nph) = (ceil_div(rw, 1 << pp).max(1), ceil_div(rh, 1 << pp).max(1));
        for py in 0..nph {
            for px in 0..npw {
                let mut pbands = Vec::new();
                for (bi, b) in bands.iter().enumerate().filter(|(_, b)| b.res == r) {
                    if b.w == 0 || b.h == 0 {
                        pbands.push(PrecinctBand { blocks_w: 0, blocks_h: 0, placed: Vec::new() });
                        continue;
                    }
                    let pband = if r == 0 { pp } else { pp.saturating_sub(1) };
                    let (cwe, che) = (p.cb_exp.0 as usize, p.cb_exp.1 as usize);
                    let (cw, ch) = (1usize << cwe.min(pband), 1usize << che.min(pband));
                    // Precinct extent in band coordinates.
                    let (pw_b, ph_b) = (1usize << pband, 1usize << pband);
                    let (bx0, by0) = (px * pw_b, py * ph_b);
                    let (bx1, by1) = (((px + 1) * pw_b).min(b.w), ((py + 1) * ph_b).min(b.h));
                    if bx0 >= bx1 || by0 >= by1 {
                        pbands.push(PrecinctBand { blocks_w: 0, blocks_h: 0, placed: Vec::new() });
                        continue;
                    }
                    let (cx0, cx1) = (bx0 / cw, ceil_div(bx1, cw));
                    let (cy0, cy1) = (by0 / ch, ceil_div(by1, ch));
                    let mut placed = Vec::new();
                    for cy in cy0..cy1 {
                        for cx in cx0..cx1 {
                            if let Some(&k) = lookup.get(&(bi, cx, cy)) {
                                placed.push(PlacedBlock { bx: cx - cx0, by: cy - cy0, block: k });
                            }
                        }
                    }
                    pbands.push(PrecinctBand { blocks_w: cx1 - cx0, blocks_h: cy1 - cy0, placed });
                }
                out.push(Precinct { res: r, x: (px << pp) << sh, y: (py << pp) << sh, bands: pbands });
            }
        }
    }
    out
}

fn packet(pr: &Precinct, c: &CompBlocks, keep: &[usize]) -> Vec<u8> {
    let mut bw = BitWriter::new();
    let mut body: Vec<u8> = Vec::new();
    let any = pr.bands.iter().any(|pb| pb.placed.iter().any(|pl| keep[pl.block] > 0));
    if !any {
        bw.put(0);
        return bw.finish();
    }
    bw.put(1);
    for pb in &pr.bands {
        if pb.placed.is_empty() {
            continue;
        }
        let mut incl = TagTree::new(pb.blocks_w, pb.blocks_h);
        let mut zbp = TagTree::new(pb.blocks_w, pb.blocks_h);
        for pl in &pb.placed {
            let cb = &c.blocks[pl.block].1;
            let n = keep[pl.block];
            incl.set(pl.bx, pl.by, if n > 0 { 0 } else { 1 });
            zbp.set(pl.bx, pl.by, cb.zero_planes);
        }
        for pl in &pb.placed {
            let cb = &c.blocks[pl.block].1;
            let n = keep[pl.block];
            incl.encode(&mut bw, pl.bx, pl.by, 1);
            if n == 0 {
                continue;
            }
            zbp.encode(&mut bw, pl.bx, pl.by, 1000);
            put_num_passes(&mut bw, n as u32);
            let len = cb.passes[n - 1].rate.min(cb.data.len());
            // A truncated codeword must not end in 0xFF.
            let mut len = len;
            if len > 0 && len < cb.data.len() && cb.data[len - 1] == 0xff {
                len -= 1;
            }
            let lblock0 = 3u32;
            let need = 32 - (len as u32).leading_zeros();
            let have = lblock0 + floor_log2(n as u32);
            let inc = need.saturating_sub(have);
            for _ in 0..inc {
                bw.put(1);
            }
            bw.put(0);
            bw.put_bits(len as u32, have + inc);
            body.extend_from_slice(&cb.data[..len]);
        }
    }
    let mut out = bw.finish();
    out.extend_from_slice(&body);
    out
}

fn main_header(p: &Params, bands: &[Band]) -> Vec<u8> {
    let mut o = vec![0xff, 0x4f];
    // SIZ
    o.extend_from_slice(&[0xff, 0x51]);
    o.extend_from_slice(&((38 + 3 * p.components as usize) as u16).to_be_bytes());
    o.extend_from_slice(&p.rsiz.to_be_bytes());
    for v in [p.width, p.height, 0, 0, p.width, p.height, 0, 0] {
        o.extend_from_slice(&v.to_be_bytes());
    }
    o.extend_from_slice(&(p.components as u16).to_be_bytes());
    for _ in 0..p.components {
        o.extend_from_slice(&[p.bit_depth - 1, 1, 1]);
    }
    // COD
    let nres = p.levels as usize + 1;
    o.extend_from_slice(&[0xff, 0x52]);
    o.extend_from_slice(&((12 + nres) as u16).to_be_bytes());
    o.push(0x01);
    o.push(p.progression as u8);
    o.extend_from_slice(&1u16.to_be_bytes());
    o.push(0);
    o.push(p.levels);
    o.push(p.cb_exp.0 - 2);
    o.push(p.cb_exp.1 - 2);
    o.push(0);
    o.push(0);
    for r in 0..nres {
        let e = p.precincts[r];
        o.push((e << 4) | e);
    }
    // QCD
    o.extend_from_slice(&[0xff, 0x5c]);
    o.extend_from_slice(&((3 + 2 * bands.len()) as u16).to_be_bytes());
    o.push((p.guard_bits << 5) | 2);
    for b in bands {
        o.extend_from_slice(&((((b.eps as u16) << 11) | b.mu as u16)).to_be_bytes());
    }
    // COM
    let text = b"Zornov JPEG 2000";
    o.extend_from_slice(&[0xff, 0x64]);
    o.extend_from_slice(&((4 + text.len()) as u16).to_be_bytes());
    o.extend_from_slice(&1u16.to_be_bytes());
    o.extend_from_slice(text);
    o
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_wavelet_is_invertible_with_the_standards_gains() {
        let mut tmp = Vec::new();
        for n in [1usize, 2, 3, 8, 9, 31, 32] {
            let orig: Vec<f32> = (0..n).map(|i| ((i * 37) % 11) as f32 - 5.0).collect();
            let mut x = orig.clone();
            fdwt_1d(&mut x, &mut tmp);
            idwt_1d(&mut x, &mut tmp);
            for (a, b) in x.iter().zip(&orig) {
                assert!((a - b).abs() < 1e-3, "n={n}: {x:?} vs {orig:?}");
            }
        }
        // DC through the low band has gain 1; Nyquist through the high band has magnitude 2.
        let mut dc = vec![1f32; 64];
        fdwt_1d(&mut dc, &mut tmp);
        assert!((dc[16] - 1.0).abs() < 1e-3, "{}", dc[16]);
        let mut ny: Vec<f32> = (0..64).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
        fdwt_1d(&mut ny, &mut tmp);
        assert!((ny[48].abs() - 2.0).abs() < 1e-3, "{}", ny[48]);
    }

    #[test]
    fn the_mq_coder_state_machine_ends_bytes_cleanly() {
        let mut mq = Mq::new();
        for i in 0..2000u32 {
            mq.encode((i % 9) as usize, ((i * 7 / 3) & 1) as u8);
        }
        mq.flush();
        assert!(mq.bp > 10);
        assert_ne!(mq.out[mq.bp], 0xff);
    }

    #[test]
    fn tag_trees_and_packet_numbers_are_coded() {
        let mut bw = BitWriter::new();
        for (n, bits) in [(1u32, 1usize), (2, 2), (3, 4), (5, 4), (6, 9), (36, 9), (37, 16), (164, 16)] {
            let before = bw.out.len() * 8 + bw.n as usize;
            put_num_passes(&mut bw, n);
            assert_eq!(bw.out.len() * 8 + bw.n as usize - before, bits, "passes {n}");
        }
        // A byte of 0xFF is followed by a 7-bit byte.
        let mut bw = BitWriter::new();
        bw.put_bits(0xff, 8);
        bw.put_bits(0x7f, 7);
        bw.put(1);
        let out = bw.finish();
        assert_eq!(out[0], 0xff);
        assert!(out[1] & 0x80 == 0);
    }

    /// A smooth test picture with edges and some noise, 12-bit XYZ-like.
    pub(crate) fn picture(w: usize, h: usize) -> [Vec<i32>; 3] {
        let mut seed = 12345u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 16) & 31) as i32 - 16
        };
        let mut planes = [vec![0i32; w * h], vec![0i32; w * h], vec![0i32; w * h]];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let base = 1000 + (x * 2000 / w) as i32 + (y * 800 / h) as i32;
                let edge = if (x / 40 + y / 40) % 2 == 0 { 600 } else { 0 };
                planes[0][i] = (base + edge + noise()).clamp(0, 4095);
                planes[1][i] = (2200 + ((x as f32 * 0.05).sin() * 600.0) as i32 + noise()).clamp(0, 4095);
                planes[2][i] = (3000 - (y * 1500 / h) as i32 + edge / 2 + noise()).clamp(0, 4095);
            }
        }
        planes
    }

    fn opj(args: &[&str]) -> Option<std::process::Output> {
        std::process::Command::new("opj_decompress").args(args).output().ok()
    }

    fn psnr(a: &[i32], b: &[i32]) -> f64 {
        let mse = a.iter().zip(b).map(|(x, y)| ((x - y) as f64).powi(2)).sum::<f64>() / a.len() as f64;
        if mse == 0.0 { 99.0 } else { 10.0 * (4095.0f64 * 4095.0 / mse).log10() }
    }

    /// Decode a codestream with OpenJPEG to a PGX-less raw: 16-bit PPM-free route via PNG is lossy for 12-bit, so use .raw.
    fn decode_with_opj(cs: &[u8], tag: &str, w: usize, h: usize) -> Option<[Vec<i32>; 3]> {
        let dir = std::env::temp_dir().join(format!("studio-j2k-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let src = dir.join("a.j2k");
        std::fs::write(&src, cs).ok()?;
        let dst = dir.join("a.pgx");
        let out = opj(&["-i", src.to_str()?, "-o", dst.to_str()?])?;
        if !out.status.success() {
            panic!("opj_decompress failed: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        }
        let mut planes = [vec![], vec![], vec![]];
        for (c, plane) in planes.iter_mut().enumerate() {
            let f = dir.join(format!("a_{c}.pgx"));
            let bytes = std::fs::read(&f).ok()?;
            // "PG ML + 12 w h\n" then 16-bit big-endian samples.
            let nl = bytes.iter().position(|&b| b == b'\n')?;
            let head = String::from_utf8_lossy(&bytes[..nl]).to_string();
            assert!(head.contains(&format!("{w} {h}")), "{head}");
            *plane = bytes[nl + 1..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]) as i32).collect();
        }
        let _ = std::fs::remove_dir_all(&dir);
        Some(planes)
    }

    #[test]
    fn openjpeg_decodes_our_codestream_to_the_picture() {
        if opj(&["-h"]).is_none() {
            eprintln!("opj_decompress not installed: skipping");
            return;
        }
        let (w, h) = (200usize, 120usize);
        let src = picture(w, h);
        let mut p = Params::general(w as u32, h as u32, 3, 12);
        p.levels = 3;
        p.precincts = vec![15; 4];
        p.step = 1.0;
        let cs = encode(&[&src[0], &src[1], &src[2]], &p).unwrap();
        let dec = decode_with_opj(&cs, "plain", w, h).expect("decoded");
        for c in 0..3 {
            let q = psnr(&dec[c], &src[c]);
            assert!(q > 46.0, "component {c}: PSNR {q:.1} dB");
        }
        assert!(cs.len() < w * h * 3 * 2, "compressed to {} bytes", cs.len());
    }

    #[test]
    fn dci_profile_with_precincts_tile_parts_and_a_byte_cap_decodes() {
        if opj(&["-h"]).is_none() {
            return;
        }
        let (w, h) = (256usize, 144usize);
        let src = picture(w, h);
        // 2K-style structure at small size: CPRL, 3 tile-parts, TLM, precincts of 64 for the upper resolutions.
        let mut p = Params::dci_2k(w as u32, h as u32, 24.0);
        p.levels = 3;
        p.precincts = vec![15, 6, 6, 6];
        p.rsiz = 0;
        p.max_bytes = None;
        let free = encode(&[&src[0], &src[1], &src[2]], &p).unwrap();
        let dec = decode_with_opj(&free, "dci", w, h).expect("decoded");
        for c in 0..3 {
            assert!(psnr(&dec[c], &src[c]) > 46.0, "component {c}: {:.1} dB", psnr(&dec[c], &src[c]));
        }
        // Capped to half: still a valid stream, smaller than the cap, lower but sane quality.
        p.max_bytes = Some(free.len() / 2);
        let capped = encode(&[&src[0], &src[1], &src[2]], &p).unwrap();
        assert!(capped.len() <= free.len() / 2, "{} vs cap {}", capped.len(), free.len() / 2);
        let dec = decode_with_opj(&capped, "cap", w, h).expect("decoded");
        for c in 0..3 {
            let q = psnr(&dec[c], &src[c]);
            assert!(q > 36.0 && q < psnr(&decode_with_opj(&free, "dci2", w, h).unwrap()[c], &src[c]), "component {c}: {q:.1} dB");
        }
    }

    #[test]
    fn odd_sizes_and_other_progressions_decode() {
        if opj(&["-h"]).is_none() {
            return;
        }
        for (w, h, prog, levels) in [(37usize, 29usize, Progression::Lrcp, 2u8), (130, 66, Progression::Rpcl, 4), (65, 97, Progression::Cprl, 3)] {
            let src = picture(w, h);
            let mut p = Params::general(w as u32, h as u32, 3, 12);
            p.levels = levels;
            p.precincts = vec![15; levels as usize + 1];
            p.progression = prog;
            let cs = encode(&[&src[0], &src[1], &src[2]], &p).unwrap();
            let dec = decode_with_opj(&cs, &format!("odd{w}"), w, h).expect("decoded");
            for c in 0..3 {
                assert!(psnr(&dec[c], &src[c]) > 44.0, "{w}x{h} {prog:?} component {c}: {:.1} dB", psnr(&dec[c], &src[c]));
            }
        }
    }
}

