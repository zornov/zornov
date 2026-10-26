// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pro still formats and image sequences, in Rust:
//!
//! * [`exr`]: OpenEXR scanline images — HALF / FLOAT / UINT channels, no,
//!   RLE, ZIPS and ZIP compression, chromaticities, time code, frame rate,
//!   the ACES container flag. Writes HALF or FLOAT, uncompressed or ZIP.
//! * [`dpx`]: SMPTE 268M DPX — 8, 10 (packed method A), 12 and 16-bit RGB /
//!   RGBA, either byte order, with time code, frame rate and the transfer
//!   characteristic. Writes 10-bit or 16-bit.
//! * [`tiff`]: baseline TIFF — 8 / 16-bit integer and 32-bit float, RGB /
//!   RGBA / grey, strips or tiles, none / LZW / Deflate / PackBits, with
//!   the horizontal predictor. Writes 16-bit RGB(A), Deflate.
//! * [`png16`]: PNG at 8 or 16 bits per sample, read and write.
//! * [`sequence`]: numbered frame sequences (`shot.1001.exr`, patterns
//!   `shot.####.exr` / `shot.%04d.exr`).
//!
//! Every reader returns an [`Image`]: RGBA `f32` samples plus what the
//! file says about how to read them ([`Encoding`], [`Meta`]).

pub mod dpx;
pub mod exr;
pub mod half;
pub mod png16;
pub mod sequence;
pub mod tiff;

use std::path::Path;

/// What the samples of an [`Image`] are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Encoding {
    /// Linear light (OpenEXR, float TIFF).
    #[default]
    Linear,
    /// Code values 0..1 of an integer format; `bits` is the file's depth.
    Code { bits: u8 },
}

/// How DPX says its values are encoded (SMPTE 268M transfer
/// characteristic), reduced to what decoding needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// Cineon printing density (10-bit log film scans).
    PrintingDensity,
    /// Linear.
    Linear,
    /// Logarithmic (also film-scan log).
    Logarithmic,
    /// BT.709 / SMPTE 274M video.
    Video,
    /// Anything else the file names, by its code.
    Other(u8),
}

/// SMPTE 12M time code read from a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TimeCode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub drop_frame: bool,
}

impl TimeCode {
    /// From packed BCD (`HH MM SS FF`, the SMPTE 12M / DPX / EXR layout,
    /// drop frame in bit 6 of the frames byte).
    pub fn from_bcd(v: u32) -> TimeCode {
        let bcd = |b: u32| ((b >> 4) & 0xF) * 10 + (b & 0xF);
        TimeCode {
            hours: bcd((v >> 24) & 0x3F) as u8,
            minutes: bcd((v >> 16) & 0x7F) as u8,
            seconds: bcd((v >> 8) & 0x7F) as u8,
            frames: bcd(v & 0x3F) as u8,
            drop_frame: v & 0x40 != 0,
        }
    }

    pub fn to_bcd(self) -> u32 {
        let bcd = |n: u8| (((n / 10) as u32) << 4) | (n % 10) as u32;
        (bcd(self.hours) << 24) | (bcd(self.minutes) << 16) | (bcd(self.seconds) << 8) | bcd(self.frames) | if self.drop_frame { 0x40 } else { 0 }
    }
}

impl std::fmt::Display for TimeCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(f, "{:02}:{:02}:{:02}{sep}{:02}", self.hours, self.minutes, self.seconds, self.frames)
    }
}

/// What a file says besides its pixels.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Meta {
    pub timecode: Option<TimeCode>,
    pub frame_rate: Option<f64>,
    /// CIE xy of red, green, blue and white (OpenEXR `chromaticities`).
    pub chromaticities: Option<[f32; 8]>,
    /// DPX transfer characteristic.
    pub transfer: Option<Transfer>,
    /// OpenEXR `acesImageContainerFlag`.
    pub aces_container: bool,
    /// A source / reel name when the file carries one.
    pub reel: Option<String>,
}

/// A decoded picture: RGBA interleaved, top row first, straight alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
    /// Whether the file had an alpha channel.
    pub has_alpha: bool,
    pub encoding: Encoding,
    pub meta: Meta,
}

impl Image {
    pub fn new(width: u32, height: u32, encoding: Encoding) -> Image {
        Image { width, height, pixels: vec![[0.0, 0.0, 0.0, 1.0]; width as usize * height as usize], has_alpha: false, encoding, meta: Meta::default() }
    }
}

/// Image formats this crate reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Exr,
    Dpx,
    Tiff,
    Png,
}

impl Format {
    /// By file extension.
    pub fn from_path(path: &Path) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "exr" => Some(Format::Exr),
            "dpx" => Some(Format::Dpx),
            "tif" | "tiff" => Some(Format::Tiff),
            "png" => Some(Format::Png),
            _ => None,
        }
    }

    /// By the first bytes of a file.
    pub fn sniff(head: &[u8]) -> Option<Format> {
        if head.starts_with(&[0x76, 0x2f, 0x31, 0x01]) {
            Some(Format::Exr)
        } else if head.starts_with(b"SDPX") || head.starts_with(b"XPDS") {
            Some(Format::Dpx)
        } else if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
            Some(Format::Tiff)
        } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Format::Png)
        } else {
            None
        }
    }
}

/// Reads any supported image by its contents.
pub fn read(path: &Path) -> Result<Image, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Decodes any supported image from memory.
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    match Format::sniff(bytes) {
        Some(Format::Exr) => exr::decode(bytes),
        Some(Format::Dpx) => dpx::decode(bytes),
        Some(Format::Tiff) => tiff::decode(bytes),
        Some(Format::Png) => png16::decode(bytes),
        None => Err("not an EXR, DPX, TIFF or PNG file".into()),
    }
}

/// Reads an image and says which format it was.
pub fn read_with_format(path: &Path) -> Result<(Image, Format), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let f = Format::sniff(&bytes).ok_or_else(|| format!("{}: not an EXR, DPX, TIFF or PNG file", path.display()))?;
    Ok((decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))?, f))
}

/// Little helpers for the binary formats.
pub(crate) struct Reader<'a> {
    pub b: &'a [u8],
    pub pos: usize,
    pub big: bool,
}

impl<'a> Reader<'a> {
    pub fn at(b: &'a [u8], pos: usize, big: bool) -> Reader<'a> {
        Reader { b, pos, big }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self.b.get(self.pos..self.pos + n).ok_or("file is truncated")?;
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        let s = self.bytes(2)?;
        Ok(if self.big { u16::from_be_bytes([s[0], s[1]]) } else { u16::from_le_bytes([s[0], s[1]]) })
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        let s = self.bytes(4)?;
        let a = [s[0], s[1], s[2], s[3]];
        Ok(if self.big { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) })
    }

    pub fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }

    pub fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        let lo = self.u32()? as u64;
        let hi = self.u32()? as u64;
        Ok(if self.big { (lo << 32) | hi } else { (hi << 32) | lo })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_bcd() {
        let tc = TimeCode { hours: 10, minutes: 59, seconds: 7, frames: 23, drop_frame: true };
        let v = tc.to_bcd();
        assert_eq!(v, 0x1059_0763);
        assert_eq!(TimeCode::from_bcd(v), tc);
        assert_eq!(tc.to_string(), "10:59:07;23");
    }
}
