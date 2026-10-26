// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The encoder: DCT, per-frame quantiser search to fill the CBR frame size,
//! entropy coding.
//!
//! Rate control: one quantiser scale factor (qsf) is found for the whole frame,
//! the smallest that fits the compressed payload; the leading macroblocks then
//! use qsf-1 for as many as still fit, so the frame is filled to the byte.

use crate::bits::{BitWriter, Counter, Put};
use crate::dct::{ZIGZAG, fdct};
use crate::header::{EOF_SIGNATURE, Header};
use crate::layout::{self, CB, CR};
use crate::vlc::Vlc;
use crate::{Chroma, Error, Frame, Params, Profile, Result, header_size};

const QMAX: u32 = 2047;

/// Encoder settings.
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub profile: Profile,
    pub width: u32,
    pub height: u32,
    /// Signal the BT.2020 non-constant-luma matrix instead of BT.709 in the header.
    pub bt2020: bool,
}

impl EncoderConfig {
    pub fn new(profile: Profile, width: u32, height: u32) -> EncoderConfig {
        EncoderConfig { profile, width, height, bt2020: false }
    }
}

/// Encodes frames of one profile and size.
pub struct Encoder {
    cfg: EncoderConfig,
    params: Params,
    vlc: &'static Vlc,
    nw: usize,
    ns: usize,
    frame_size: usize,
    hs: usize,
}

/// Runs `f` for 0..n, in parallel with the `threads` feature. The results come back in order.
fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync + Send) -> Vec<T> {
    #[cfg(feature = "threads")]
    {
        use rayon::prelude::*;
        (0..n).into_par_iter().map(f).collect()
    }
    #[cfg(not(feature = "threads"))]
    {
        (0..n).map(f).collect()
    }
}

/// Quantises one block (zig-zag order, DC excluded) with reciprocal step sizes.
#[inline]
fn quantize(coefs: &[f32], inv: &[f32; 64], max_amp: i32, out: &mut [i32; 64]) {
    out[0] = 0;
    for r in 1..63 {
        let x = coefs[r];
        let n = ((x.abs() * inv[r]) as i32).min(max_amp);
        out[r] = if x < 0.0 { -n } else { n };
    }
    // A coefficient at the last position would make the end-of-block code ambiguous.
    out[63] = 0;
}

/// Writes the AC codewords and the end-of-block code of one block.
#[inline]
fn emit_ac<P: Put>(p: &mut P, vlc: &Vlc, pbits: u32, q: &[i32; 64]) {
    let mut run = 0usize;
    for &c in &q[1..] {
        if c == 0 {
            run += 1;
            continue;
        }
        let a = c.unsigned_abs() as usize;
        let (base, pidx, index) = if a <= 64 { (a, 0, false) } else { (a - 64 * ((a - 1) >> 6), (a - 1) >> 6, true) };
        let (code, len) = vlc.amp_enc[(run > 0) as usize][index as usize][base];
        p.put(code as u32, len as u32);
        p.put((c < 0) as u32, 1);
        if index {
            p.put(pidx as u32, pbits);
        }
        if run > 0 {
            let (rc, rl) = vlc.run_enc[run];
            p.put(rc as u32, rl as u32);
        }
        run = 0;
    }
    p.put(vlc.eob.0 as u32, vlc.eob.1 as u32);
}

#[inline]
fn emit_dc<P: Put>(p: &mut P, vlc: &Vlc, diff: i32) {
    let a = diff.unsigned_abs();
    let eta = (32 - a.leading_zeros()) as usize;
    let (code, len) = vlc.dc_enc[eta];
    p.put(code as u32, len as u32);
    if eta > 0 {
        let rho = if diff > 0 { diff as u32 } else { (diff + (1 << eta) - 1) as u32 };
        p.put(rho, eta as u32);
    }
}

/// Reciprocal step sizes 32 / (qsf * W) per zig-zag position.
fn inverse_steps(weights: &[u8; 64], qsf: u32) -> [f32; 64] {
    let mut inv = [0f32; 64];
    for r in 1..64 {
        inv[r] = 32.0 / (qsf as f32 * weights[ZIGZAG[r] as usize] as f32);
    }
    inv
}

struct Analysis {
    nblocks: usize,
    /// NT * nblocks * 64 coefficients, zig-zag order; slot 0 of each block is the DC value.
    coefs: Vec<f32>,
    /// Bits of everything that does not depend on qsf, per macroblock.
    fixed_bits: Vec<u32>,
    dc: Vec<i32>,
}

impl Encoder {
    pub fn new(cfg: EncoderConfig) -> Result<Encoder> {
        Encoder::with_config(cfg)
    }

    pub fn with_config(cfg: EncoderConfig) -> Result<Encoder> {
        if cfg.width == 0 || cfg.height == 0 || cfg.width > 16384 || cfg.height > 16384 {
            return Err(Error::Input(format!("{}x{} is outside 1..=16384", cfg.width, cfg.height)));
        }
        if cfg.profile.chroma() == Chroma::Yuv422 && cfg.width % 2 != 0 {
            return Err(Error::Input("4:2:2 needs an even width".into()));
        }
        let params = cfg.profile.params();
        let frame_size = cfg.profile.frame_size(cfg.width, cfg.height);
        let hs = header_size(cfg.height);
        Ok(Encoder {
            vlc: Vlc::get(params.set),
            nw: cfg.width.div_ceil(16) as usize,
            ns: cfg.height.div_ceil(16) as usize,
            params,
            frame_size,
            hs,
            cfg,
        })
    }

    /// Size in bytes of every frame this encoder writes.
    pub fn frame_size(&self) -> usize {
        self.frame_size
    }

    fn pbits(&self) -> u32 {
        if self.cfg.profile.bit_depth() == 8 { 4 } else { 6 }
    }

    fn max_amp(&self) -> i32 {
        if self.cfg.profile.bit_depth() == 8 { 1024 } else { 4096 }
    }

    fn weights(&self, ct: usize) -> &'static [u8; 64] {
        if ct == CB || ct == CR { self.params.chroma } else { self.params.luma }
    }

    /// Encodes one frame.
    pub fn encode(&self, f: &Frame) -> Result<Vec<u8>> {
        let p = self.cfg.profile;
        if (f.width, f.height) != (self.cfg.width, self.cfg.height) || f.chroma != p.chroma() || f.bit_depth != p.bit_depth() {
            return Err(Error::Input(format!("{} wants {}x{} {:?} at {} bits", p.name(), self.cfg.width, self.cfg.height, p.chroma(), p.bit_depth())));
        }
        let cw = f.chroma_width() as usize;
        if f.y.len() != (f.width * f.height) as usize || f.cb.len() != cw * f.height as usize || f.cr.len() != f.cb.len() {
            return Err(Error::Input("plane sizes do not match the frame size".into()));
        }
        let a = self.analyse(f);
        let payload = self.frame_size - self.hs - 4;
        let (q, split) = self.search(&a, payload)?;
        // The scan lines.
        let lines: Vec<Vec<u8>> = par_map(self.ns, |s| self.write_line(&a, s, q, split));
        let mut out = vec![0u8; self.hs];
        let mut offsets = Vec::with_capacity(self.ns);
        let mut at = 0usize;
        for l in &lines {
            offsets.push(at as u32);
            at += l.len();
        }
        if at > payload {
            return Err(Error::Input("internal: coded data exceeds the frame size".into()));
        }
        for l in &lines {
            out.extend_from_slice(l);
        }
        out.resize(self.frame_size - 4, 0);
        out.extend_from_slice(&EOF_SIGNATURE);
        let h = Header {
            header_size: self.hs as u32,
            version: 3,
            vbr: false,
            progressive_frame: true,
            crc_flag: false,
            width: f.width,
            height: f.height,
            pixel_aspect: (1, 1),
            bit_depth_code: if p.bit_depth() == 8 { 1 } else { 2 },
            interlaced_source: false,
            compression_id: p.cid(),
            frame_encoding: true,
            sub_sampling: if p.chroma() == Chroma::Yuv444 { 2 } else { 0 },
            // 4:4:4 is coded as RGB; its samples are stored as given, with no matrix.
            // 4:4:4 is signalled as RGB with every macroblock coded as Y'CbCr (ACF = 1), which
            // a decoder converts back to RGB; the planes are Y', Cb and Cr like the other profiles.
            colour_volume: self.cfg.bt2020 as u8,
            rgb: p == Profile::P444,
            timecode_present: false,
            scan_lines: self.ns as u32,
            scan_offsets: offsets,
        };
        h.write(&mut out[..self.hs]);
        Ok(out)
    }

    /// DCT of every block and the qsf-independent part of the cost.
    fn analyse(&self, f: &Frame) -> Analysis {
        let chroma = self.cfg.profile.chroma();
        let blocks = layout::blocks(chroma);
        let nb = blocks.len();
        let bits = f.bit_depth as u32;
        let shift = 1i32 << (bits - 1);
        let maxv = (1i32 << bits) - 1;
        let cw = f.chroma_width() as usize;
        let (w, h) = (f.width as usize, f.height as usize);
        let plane = |ct: usize| -> (&[u16], usize, usize) {
            match ct {
                0 => (&f.y[..], w, h),
                1 => (&f.cb[..], cw, h),
                _ => (&f.cr[..], cw, h),
            }
        };
        let vlc = self.vlc;
        let lines: Vec<(Vec<f32>, Vec<i32>, Vec<u32>)> = par_map(self.ns, |s| {
            let mut coefs = vec![0f32; self.nw * nb * 64];
            let mut dcs = vec![0i32; self.nw * nb];
            let mut fixed = vec![0u32; self.nw];
            let mut pred = [0i32; 3];
            for m in 0..self.nw {
                let mut fx = 12u32;
                for (k, &(ct, dx, dy)) in blocks.iter().enumerate() {
                    let (pl, pw, ph) = plane(ct);
                    let ox = m * layout::mb_width(chroma, ct) + dx;
                    let oy = s * 16 + dy;
                    let mut x = [0f64; 64];
                    for j in 0..8 {
                        let yy = (oy + j).min(ph - 1);
                        for i in 0..8 {
                            let xx = (ox + i).min(pw - 1);
                            x[j * 8 + i] = ((pl[yy * pw + xx] as i32).min(maxv) - shift) as f64;
                        }
                    }
                    let c = fdct(&x);
                    let base = (m * nb + k) * 64;
                    for r in 0..64 {
                        coefs[base + r] = c[ZIGZAG[r] as usize] as f32;
                    }
                    let dc = c[0].round() as i32;
                    dcs[m * nb + k] = dc;
                    coefs[base] = dc as f32;
                    let mut cnt = Counter::default();
                    emit_dc(&mut cnt, vlc, dc - pred[ct]);
                    pred[ct] = dc;
                    fx += cnt.0 + vlc.eob.1 as u32;
                }
                fixed[m] = fx;
            }
            (coefs, dcs, fixed)
        });
        let mut an = Analysis { nblocks: nb, coefs: Vec::new(), fixed_bits: Vec::new(), dc: Vec::new() };
        for (c, d, fx) in lines {
            an.coefs.extend_from_slice(&c);
            an.dc.extend_from_slice(&d);
            an.fixed_bits.extend_from_slice(&fx);
        }
        an
    }

    /// Bits of every macroblock at a given qsf.
    fn costs(&self, a: &Analysis, qsf: u32) -> Vec<u32> {
        let chroma = self.cfg.profile.chroma();
        let blocks = layout::blocks(chroma);
        let inv_l = inverse_steps(self.weights(0), qsf);
        let inv_c = inverse_steps(self.weights(CB), qsf);
        let (vlc, pbits, max_amp) = (self.vlc, self.pbits(), self.max_amp());
        par_map(self.ns, |s| {
            let mut out = vec![0u32; self.nw];
            let mut qv = [0i32; 64];
            for m in 0..self.nw {
                let mb = s * self.nw + m;
                let mut cnt = Counter(a.fixed_bits[mb]);
                for (k, &(ct, _, _)) in blocks.iter().enumerate() {
                    let base = (mb * a.nblocks + k) * 64;
                    quantize(&a.coefs[base..base + 64], if ct == 0 { &inv_l } else { &inv_c }, max_amp, &mut qv);
                    emit_ac(&mut cnt, vlc, pbits, &qv);
                }
                out[m] = cnt.0;
            }
            out
        })
        .concat()
    }

    /// Bytes of the compressed payload for per-macroblock costs.
    fn payload_bytes(&self, costs: &[u32]) -> usize {
        costs.chunks(self.nw).map(|l| l.iter().map(|&b| b as usize).sum::<usize>().div_ceil(32) * 4).sum()
    }

    /// The frame's qsf and how many leading macroblocks use qsf-1 instead.
    fn search(&self, a: &Analysis, payload: usize) -> Result<(u32, usize)> {
        let fits = |q: u32| self.payload_bytes(&self.costs(a, q)) <= payload;
        if fits(1) {
            return Ok((1, 0));
        }
        if !fits(QMAX) {
            return Err(Error::Input("picture does not fit the frame size even at the coarsest quantiser".into()));
        }
        let (mut lo, mut hi) = (1u32, QMAX); // lo does not fit, hi fits
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if fits(mid) { hi = mid } else { lo = mid }
        }
        let q = hi;
        let fine = self.costs(a, q - 1);
        let coarse = self.costs(a, q);
        let total = |k: usize| -> usize {
            let mixed: Vec<u32> = (0..coarse.len()).map(|i| if i < k { fine[i] } else { coarse[i] }).collect();
            self.payload_bytes(&mixed)
        };
        let (mut lo_k, mut hi_k) = (0usize, coarse.len()); // lo_k fits; find the largest that does
        if total(hi_k) <= payload {
            return Ok((q, hi_k));
        }
        while hi_k - lo_k > 1 {
            let mid = (lo_k + hi_k) / 2;
            if total(mid) <= payload { lo_k = mid } else { hi_k = mid }
        }
        Ok((q, lo_k))
    }

    /// One macroblock scan line, padded to a multiple of four bytes.
    fn write_line(&self, a: &Analysis, s: usize, q: u32, split: usize) -> Vec<u8> {
        let chroma = self.cfg.profile.chroma();
        let blocks = layout::blocks(chroma);
        let (vlc, pbits, max_amp) = (self.vlc, self.pbits(), self.max_amp());
        let acf = (self.cfg.profile == Profile::P444) as u32;
        let mut w = BitWriter::new();
        let mut pred = [0i32; 3];
        let mut qv = [0i32; 64];
        let mut cur_q = 0u32;
        let (mut inv_l, mut inv_c) = ([0f32; 64], [0f32; 64]);
        for m in 0..self.nw {
            let mb = s * self.nw + m;
            let qsf = if mb < split { q - 1 } else { q };
            if qsf != cur_q {
                cur_q = qsf;
                inv_l = inverse_steps(self.weights(0), qsf);
                inv_c = inverse_steps(self.weights(CB), qsf);
            }
            w.put(qsf << 1 | acf, 12);
            for (k, &(ct, _, _)) in blocks.iter().enumerate() {
                let dc = a.dc[mb * a.nblocks + k];
                emit_dc(&mut w, vlc, dc - pred[ct]);
                pred[ct] = dc;
                let base = (mb * a.nblocks + k) * 64;
                quantize(&a.coefs[base..base + 64], if ct == 0 { &inv_l } else { &inv_c }, max_amp, &mut qv);
                emit_ac(&mut w, vlc, pbits, &qv);
            }
        }
        w.finish_aligned()
    }
}
