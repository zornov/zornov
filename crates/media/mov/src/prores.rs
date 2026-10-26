// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! RGB(A) pictures → ProRes frames.
//!
//! Input is encoded R′G′B′(A) at 16 bits (0..65535 full scale), as
//! render's 16-bit output hands it out. It is converted to Y′CbCr with the
//! BT.709 or BT.2020 (non-constant luminance) matrix in limited range:
//! 10-bit for the 422 profiles, chroma halved horizontally with a
//! [1 2 1] / 4 filter on co-sited samples; 12-bit 4:4:4 for 4444 / XQ,
//! with alpha when asked.

use studio_prores::{ChromaFormat, ColorInfo, Encoder, EncoderConfig, Frame, Profile};

/// Which Y′CbCr matrix to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt709,
    Bt2020,
}

impl Matrix {
    pub(crate) fn kr_kb(self) -> (f32, f32) {
        match self {
            Matrix::Bt709 => (0.2126, 0.0722),
            Matrix::Bt2020 => (0.2627, 0.0593),
        }
    }
}

/// Encodes 16-bit RGB(A) frames as ProRes.
pub struct ProResEncoder {
    enc: Encoder,
    profile: Profile,
    matrix: Matrix,
    alpha: bool,
}

impl ProResEncoder {
    /// `colour` holds the H.273 primaries, transfer and matrix written in
    /// the frame header (and in the file's `colr` atom).
    pub fn new(profile: Profile, width: u32, height: u32, colour: (u8, u8, u8), alpha: bool) -> ProResEncoder {
        let mut cfg = EncoderConfig::new(profile, width, height);
        cfg.color = ColorInfo { primaries: colour.0, transfer: colour.1, matrix: colour.2 };
        cfg.encode_alpha = alpha && profile.chroma() == ChromaFormat::Yuv444;
        cfg.encoder_id = *b"stdo";
        let matrix = if colour.2 == 9 { Matrix::Bt2020 } else { Matrix::Bt709 };
        ProResEncoder { enc: Encoder::with_config(cfg), profile, matrix, alpha: alpha && profile.chroma() == ChromaFormat::Yuv444 }
    }

    /// The Y′CbCr frame for an RGBA16 picture.
    pub fn frame(&self, rgba: &[u16], width: u32, height: u32) -> Frame {
        let chroma = self.profile.chroma();
        let bits: u8 = if chroma == ChromaFormat::Yuv444 { 12 } else { 10 };
        let mut f = Frame::new(width, height, chroma, bits, self.alpha);
        let (kr, kb) = self.matrix.kr_kb();
        let kg = 1.0 - kr - kb;
        let s = (1u32 << (bits - 8)) as f32;
        let (y_off, y_scale, c_off, c_scale) = (16.0 * s, 219.0 * s, 128.0 * s, 224.0 * s);
        let max = ((1u32 << bits) - 1) as f32;
        let (w, h) = (width as usize, height as usize);
        let mut cb_full = vec![0f32; w];
        let mut cr_full = vec![0f32; w];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let (r, g, b) = (rgba[i] as f32 / 65535.0, rgba[i + 1] as f32 / 65535.0, rgba[i + 2] as f32 / 65535.0);
                let yy = kr * r + kg * g + kb * b;
                f.y[y * w + x] = (y_off + y_scale * yy).round().clamp(0.0, max) as u16;
                cb_full[x] = (b - yy) / (2.0 * (1.0 - kb));
                cr_full[x] = (r - yy) / (2.0 * (1.0 - kr));
                if let Some(a) = &mut f.alpha {
                    a[y * w + x] = (rgba[i + 3] as f32 / 65535.0 * max).round() as u16;
                }
            }
            let cw = f.chroma_width() as usize;
            for cx in 0..cw {
                let (cb, cr) = if chroma == ChromaFormat::Yuv444 {
                    (cb_full[cx], cr_full[cx])
                } else {
                    let x = cx * 2;
                    let l = x.saturating_sub(1);
                    let r = (x + 1).min(w - 1);
                    ((cb_full[l] + 2.0 * cb_full[x] + cb_full[r]) / 4.0, (cr_full[l] + 2.0 * cr_full[x] + cr_full[r]) / 4.0)
                };
                f.cb[y * cw + cx] = (c_off + c_scale * cb).round().clamp(0.0, max) as u16;
                f.cr[y * cw + cx] = (c_off + c_scale * cr).round().clamp(0.0, max) as u16;
            }
        }
        f
    }

    /// Encodes one RGBA16 picture.
    pub fn encode(&mut self, rgba: &[u16], width: u32, height: u32) -> Result<Vec<u8>, String> {
        let f = self.frame(rgba, width, height);
        self.enc.encode(&f).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_and_colour_survive_a_round_trip() {
        let (w, h) = (64u32, 32u32);
        let mut rgba = vec![0u16; (w * h * 4) as usize];
        for (i, p) in rgba.chunks_exact_mut(4).enumerate() {
            let x = (i as u32 % w) as f32 / w as f32;
            p.copy_from_slice(&[(x * 65535.0) as u16, 30000, 20000, 65535]);
        }
        for profile in [Profile::Hq, Profile::P4444] {
            let mut e = ProResEncoder::new(profile, w, h, (1, 1, 1), true);
            let bytes = e.encode(&rgba, w, h).unwrap();
            let f = studio_prores::decode_frame(&bytes).unwrap();
            assert_eq!((f.width, f.height), (w, h));
            // Luma at the centre is close to what went in.
            let want = e.frame(&rgba, w, h);
            let i = (16 * w + 32) as usize;
            assert!((f.y[i] as i32 - want.y[i] as i32).abs() <= 6 << (f.bit_depth - 10), "{profile:?}: {} vs {}", f.y[i], want.y[i]);
            assert_eq!(f.alpha.is_some(), profile == Profile::P4444);
        }
    }
}
