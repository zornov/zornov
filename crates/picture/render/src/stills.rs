// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pro stills and frame sequences as sources: OpenEXR, DPX, TIFF and
//! 16-bit PNG (read by `studio-imageio`), one file or a numbered sequence
//! (`shot.####.exr`).
//!
//! Pictures travel as 16-bit full-range RGB planes, so they take the same
//! GPU path as video:
//!
//! * integer formats keep their code values; DPX printing density / log
//!   decodes with the Cineon curve, DPX video with BT.1886, DPX linear as
//!   linear, TIFF and PNG as sRGB (an input colour space overrides this);
//! * linear float images (EXR, float TIFF) are stored ACEScct-encoded,
//!   which holds −0.007 … 222 in 16 bits with finer steps than half
//!   floats in the shadows, and decode back to linear on the way in. Their
//!   primaries come from the file's chromaticities (ACES AP0 / AP1,
//!   Rec.2020, else Rec.709).
//!
//! Alpha channels are read but not drawn yet (the picture is opaque).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use studio_colour::camera::{Gamut, LogCurve};
use studio_imageio::sequence::Sequence;
use studio_imageio::{Encoding, Image, Transfer as FileTransfer};

use crate::colour_space::{ColourDesc, InputColour, Matrix, SourcePrimaries, SourceTransfer};
use crate::scene::Micros;
use crate::source::{MediaSource, SourceInfo};
use crate::yuv::{DecodedPicture, PictureRef, PixelData, Planes};

/// Whether a file is one of the formats handled here (by its first bytes).
pub fn sniff(head: &[u8]) -> bool {
    matches!(studio_imageio::Format::sniff(head), Some(studio_imageio::Format::Exr | studio_imageio::Format::Dpx | studio_imageio::Format::Tiff))
}

/// Whether a PNG is 16 bits per sample (kept at full precision here).
pub fn is_png16(head: &[u8]) -> bool {
    head.len() >= 26 && head.starts_with(b"\x89PNG\r\n\x1a\n") && head[24] == 16
}

fn close(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 0.005 && (a[1] - b[1]).abs() < 0.005
}

/// The colour description a file implies.
pub fn colour_of(img: &Image) -> ColourDesc {
    let base = ColourDesc { primaries: SourcePrimaries::Bt709, transfer: SourceTransfer::Srgb, matrix: Matrix::Rgb, full_range: true, bit_depth: 16 };
    match img.encoding {
        Encoding::Linear => {
            let primaries = match img.meta.chromaticities {
                Some(c) if close([c[0], c[1]], [0.7347, 0.2653]) && close([c[2], c[3]], [0.0, 1.0]) => SourcePrimaries::Gamut(Gamut::AcesAp0),
                Some(c) if close([c[0], c[1]], [0.713, 0.293]) => SourcePrimaries::Gamut(Gamut::AcesAp1),
                Some(c) if close([c[0], c[1]], [0.708, 0.292]) => SourcePrimaries::Bt2020,
                Some(c) if close([c[0], c[1]], [0.68, 0.32]) => SourcePrimaries::P3D65,
                _ if img.meta.aces_container => SourcePrimaries::Gamut(Gamut::AcesAp0),
                _ => SourcePrimaries::Bt709,
            };
            ColourDesc { primaries, transfer: SourceTransfer::Camera { curve: LogCurve::AcesCct, legal: false }, ..base }
        }
        Encoding::Code { .. } => {
            let transfer = match img.meta.transfer {
                Some(FileTransfer::PrintingDensity | FileTransfer::Logarithmic) => SourceTransfer::Camera { curve: LogCurve::Cineon, legal: false },
                Some(FileTransfer::Video) => SourceTransfer::Bt709,
                Some(FileTransfer::Linear) => SourceTransfer::Linear,
                _ => SourceTransfer::Srgb,
            };
            ColourDesc { transfer, ..base }
        }
    }
}

/// A decoded file as a picture (see the module notes).
pub fn picture_of(img: &Image) -> DecodedPicture {
    let desc = colour_of(img);
    let linear = img.encoding == Encoding::Linear;
    let q = |v: f32| -> u16 {
        let c = if linear { studio_colour::transfer::lin_to_acescct(v) } else { v };
        (c.clamp(0.0, 1.0) * 65535.0).round() as u16
    };
    let n = img.pixels.len();
    let (mut y, mut u, mut v) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for p in &img.pixels {
        y.push(q(p[1]));
        u.push(q(p[2]));
        v.push(q(p[0]));
    }
    DecodedPicture {
        width: img.width,
        height: img.height,
        data: PixelData::Yuv { y: Planes::U16(y), u: Planes::U16(u), v: Planes::U16(v), chroma_width: img.width, chroma_height: img.height },
        colour: desc,
    }
}

fn read_picture(path: &Path, input: Option<InputColour>) -> Result<(DecodedPicture, studio_imageio::Meta), String> {
    let img = studio_imageio::read(path)?;
    let mut pic = picture_of(&img);
    if let Some(i) = input {
        pic.colour = i.apply(pic.colour);
    }
    Ok((pic, img.meta))
}

/// One still file.
pub fn open_still(path: &Path, input: Option<InputColour>) -> Result<crate::source::StillSource, String> {
    Ok(crate::source::StillSource::new(read_picture(path, input)?.0))
}

/// A numbered frame sequence, read a frame at a time (frames are
/// independent, so any frame decodes directly). A missing frame shows the
/// nearest earlier one.
pub struct SequenceSource {
    seq: Sequence,
    input: Option<InputColour>,
    rate: f64,
    width: u32,
    height: u32,
    colour: ColourDesc,
}

impl SequenceSource {
    /// Opens `path`, a `#` pattern or any frame of the sequence. `rate`
    /// overrides the frame rate the files carry (default 24).
    pub fn open(path: &Path, input: Option<InputColour>, rate: Option<f64>) -> Result<SequenceSource, String> {
        let seq = if Sequence::is_pattern(path) { Sequence::from_pattern(path)? } else { Sequence::from_file(path)? };
        let first = (seq.first..=seq.last).map(|n| seq.frame(n)).find(|p| p.exists()).ok_or("the sequence has no readable frame")?;
        let (pic, meta) = read_picture(&first, input)?;
        let rate = rate.or(meta.frame_rate).filter(|r| *r > 0.0).unwrap_or(24.0);
        Ok(SequenceSource { width: pic.width, height: pic.height, colour: pic.colour, seq, input, rate })
    }

    pub fn sequence(&self) -> &Sequence {
        &self.seq
    }

    fn file(&self, index: u64) -> Option<PathBuf> {
        let mut n = self.seq.first + index as i64;
        if n > self.seq.last {
            return None;
        }
        while self.seq.missing.binary_search(&n).is_ok() && n > self.seq.first {
            n -= 1;
        }
        Some(self.seq.frame(n))
    }
}

impl MediaSource for SequenceSource {
    fn info(&self) -> SourceInfo {
        let frames = self.seq.len() as u64;
        SourceInfo {
            width: self.width,
            height: self.height,
            rotation: 0,
            frame_rate: Some(self.rate),
            duration: Some((frames as f64 / self.rate * 1e6).round() as Micros),
            frame_count: Some(frames),
            colour: self.colour,
        }
    }

    fn frame_index(&self, time: Micros) -> u64 {
        let i = (time.max(0) as f64 / 1e6 * self.rate + 1e-6).floor() as u64;
        i.min(self.seq.len().saturating_sub(1) as u64)
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        let Some(path) = self.file(index) else { return false };
        match read_picture(&path, self.input) {
            Ok((pic, _)) => {
                emit(index, Arc::new(pic));
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exr_frames_decode_to_their_linear_values() {
        let dir = std::env::temp_dir().join(format!("studio-stills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in 0..3 {
            let v = 0.18 * (n + 1) as f32;
            let px = vec![[v, v * 2.0, 30.0, 1.0]; 8 * 4];
            let bytes = studio_imageio::exr::encode(8, 4, &px, &studio_imageio::exr::WriteOptions { frame_rate: Some((25, 1)), ..Default::default() });
            std::fs::write(dir.join(format!("plate.{:04}.exr", 1001 + n)), bytes).unwrap();
        }
        let mut s = SequenceSource::open(&dir.join("plate.####.exr"), None, None).unwrap();
        let info = s.info();
        assert_eq!((info.width, info.height, info.frame_count, info.frame_rate), (8, 4, Some(3), Some(25.0)));
        assert_eq!(s.frame_index(SECOND_TEST / 25 * 2), 2);
        let mut got = None;
        assert!(s.decode_to(1, &mut |_, p| got = Some(p)));
        let f = got.unwrap().to_frame(false);
        let p = f.pixels[5];
        assert!((p[0] - 0.36).abs() < 0.002 && (p[1] - 0.72).abs() < 0.003 && (p[2] - 30.0).abs() < 0.2, "{p:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    const SECOND_TEST: Micros = 1_000_000;

    #[test]
    fn dpx_log_uses_cineon() {
        let px = vec![[685.0 / 1023.0, 95.0 / 1023.0, 0.5, 1.0]; 4];
        let bytes = studio_imageio::dpx::encode(2, 2, &px, &studio_imageio::dpx::WriteOptions { transfer: FileTransfer::PrintingDensity, ..Default::default() });
        let img = studio_imageio::decode(&bytes).unwrap();
        let pic = picture_of(&img);
        assert_eq!(pic.colour.transfer, SourceTransfer::Camera { curve: LogCurve::Cineon, legal: false });
        let p = pic.to_frame(false).pixels[0];
        assert!((p[0] - 1.0).abs() < 0.01 && p[1].abs() < 0.01, "{p:?}");
    }
}
