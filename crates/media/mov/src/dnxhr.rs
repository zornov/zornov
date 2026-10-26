// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! RGB(A) pictures → DNxHR frames.
//!
//! Input is encoded R′G′B′(A) at 16 bits (0..65535 full scale), as render's
//! 16-bit output hands it out. It is converted to limited-range Y′CbCr with the
//! BT.709 or BT.2020 (non-constant luminance) matrix: 8-bit for LB, SQ and HQ,
//! 10-bit for HQX and 444. Chroma of the 4:2:2 profiles is halved
//! horizontally with a [1 2 1] / 4 filter on co-sited samples. Alpha is dropped.

use crate::prores::Matrix;
use studio_dnxhr::{Chroma, Encoder, EncoderConfig, Frame};

pub use studio_dnxhr::Profile;

/// Encodes 16-bit RGB(A) frames as DNxHR.
pub struct DnxEncoder {
    enc: Encoder,
    profile: Profile,
    matrix: Matrix,
}

impl DnxEncoder {
    /// `colour` holds the H.273 primaries, transfer and matrix (the matrix
    /// picks BT.709 or BT.2020 and is signalled in the frame header).
    /// Fails for an odd width with a 4:2:2 profile, or a size outside 1..=16384.
    pub fn new(profile: Profile, width: u32, height: u32, colour: (u8, u8, u8)) -> Result<DnxEncoder, String> {
        let bt2020 = colour.2 == 9;
        let mut cfg = EncoderConfig::new(profile, width, height);
        cfg.bt2020 = bt2020;
        let enc = Encoder::new(cfg).map_err(|e| e.to_string())?;
        Ok(DnxEncoder { enc, profile, matrix: if bt2020 { Matrix::Bt2020 } else { Matrix::Bt709 } })
    }

    /// Bytes of every frame this encoder writes.
    pub fn frame_size(&self) -> usize {
        self.enc.frame_size()
    }

    /// The Y′CbCr frame for an RGBA16 picture.
    pub fn frame(&self, rgba: &[u16], width: u32, height: u32) -> Frame {
        let chroma = self.profile.chroma();
        let bits = self.profile.bit_depth();
        let mut f = Frame::new(width, height, chroma, bits);
        let (kr, kb) = self.matrix.kr_kb();
        let kg = 1.0 - kr - kb;
        let s = (1u32 << (bits - 8)) as f32;
        let (y_off, y_scale, c_off, c_scale) = (16.0 * s, 219.0 * s, 128.0 * s, 224.0 * s);
        let max = ((1u32 << bits) - 1) as f32;
        let (w, h) = (width as usize, height as usize);
        let cw = f.chroma_width() as usize;
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
            }
            for cx in 0..cw {
                let (cb, cr) = if chroma == Chroma::Yuv444 {
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
        if rgba.len() != (width as usize) * (height as usize) * 4 {
            return Err("picture size does not match its pixel count".into());
        }
        let f = self.frame(rgba, width, height);
        self.enc.encode(&f).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mov::{MovWriter, VideoTrack};

    fn picture(w: u32, h: u32) -> Vec<u16> {
        let mut rgba = vec![0u16; (w * h * 4) as usize];
        for (i, p) in rgba.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            p.copy_from_slice(&[(x * 65535 / w) as u16, (y * 65535 / h) as u16, 20000, 65535]);
        }
        rgba
    }

    #[test]
    fn rgb_survives_a_round_trip_for_every_profile() {
        let (w, h) = (96u32, 64u32);
        let rgba = picture(w, h);
        for profile in Profile::ALL {
            let mut e = DnxEncoder::new(profile, w, h, (1, 1, 1)).unwrap();
            let bytes = e.encode(&rgba, w, h).unwrap();
            assert_eq!(bytes.len(), e.frame_size());
            let got = studio_dnxhr::decode_frame(&bytes).unwrap();
            let want = e.frame(&rgba, w, h);
            let i = (32 * w + 48) as usize;
            let tol = 6 << (profile.bit_depth() - 8);
            assert!((got.y[i] as i32 - want.y[i] as i32).abs() <= tol, "{profile:?}: {} vs {}", got.y[i], want.y[i]);
        }
    }

    #[test]
    fn a_quicktime_file_carries_avdh_frames_and_audio() {
        let (w, h) = (64u32, 48u32);
        let rgba = picture(w, h);
        let dir = std::env::temp_dir().join(format!("studio-mov-dnx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.mov");
        let mut e = DnxEncoder::new(Profile::Hqx, w, h, (1, 1, 1)).unwrap();
        let video = VideoTrack { profile: studio_prores::Profile::Hq, width: w, height: h, rate_num: 24, rate_den: 1, colour: (1, 1, 1), alpha: false };
        let mut m = MovWriter::create_dnxhr(&path, Profile::Hqx, video, Some(crate::mov::AudioTrack { sample_rate: 48000, channels: 2 }), None).unwrap();
        for _ in 0..3 {
            m.write_video(&e.encode(&rgba, w, h).unwrap()).unwrap();
            m.write_audio(&vec![0.25f32; 2000 * 2]).unwrap();
        }
        m.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let find = |pat: &[u8]| bytes.windows(pat.len()).position(|w| w == pat);
        assert!(find(b"AVdh").is_some(), "sample entry");
        assert!(find(b"apch").is_none());
        assert!(find(b"DNxHR HQX").is_some(), "compressor name");
        // Frame sample sizes sit in the stsz table: three equal entries.
        let stsz = find(b"stsz").unwrap();
        let sizes: Vec<u32> = (0..3).map(|k| u32::from_be_bytes(bytes[stsz + 16 + 4 * k..stsz + 20 + 4 * k].try_into().unwrap())).collect();
        assert!(sizes.iter().all(|&s| s as usize == e.frame_size()), "{sizes:?}");
        // The first frame in mdat decodes.
        let first = find(b"mdat").unwrap() + 4 + 8;
        let frame = studio_dnxhr::decode_frame(&bytes[first..first + e.frame_size()]).unwrap();
        assert_eq!((frame.width, frame.height), (w, h));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
