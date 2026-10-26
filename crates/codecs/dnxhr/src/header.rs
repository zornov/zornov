// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The coding-unit header (ST 2019-1 section 7.2, Table 8).

use crate::{Error, Result, header_size};

/// Header fields this crate reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Header size recorded in the prefix (HS).
    pub header_size: u32,
    /// Header version number (3 for the resolution-independent profiles).
    pub version: u8,
    pub vbr: bool,
    pub progressive_frame: bool,
    pub crc_flag: bool,
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: (u32, u32),
    /// Sample bit depth code (1 = 8-bit, 2 = 10-bit, 3 = 12-bit).
    pub bit_depth_code: u8,
    pub interlaced_source: bool,
    pub compression_id: u32,
    pub frame_encoding: bool,
    /// 0 = 4:2:2, 1 = 4:2:0, 2 = 4:4:4.
    pub sub_sampling: u8,
    /// 0 = BT.709, 1 = BT.2020 non-constant luma, 2 = BT.2020 constant luma, 3 = out of band.
    pub colour_volume: u8,
    pub rgb: bool,
    pub timecode_present: bool,
    /// Number of macroblock scan line indices (NS).
    pub scan_lines: u32,
    /// Byte offset of each macroblock scan line, relative to the compressed payload.
    pub scan_offsets: Vec<u32>,
}

pub const EOF_SIGNATURE: [u8; 4] = [0x60, 0x0D, 0xC0, 0xDE];

impl Header {
    pub fn bit_depth(&self) -> u8 {
        match self.bit_depth_code {
            1 => 8,
            2 => 10,
            _ => 12,
        }
    }

    pub(crate) fn write(&self, out: &mut [u8]) {
        let be16 = |out: &mut [u8], at: usize, v: u32| out[at..at + 2].copy_from_slice(&(v as u16).to_be_bytes());
        out[0..4].copy_from_slice(&self.header_size.to_be_bytes());
        out[4] = self.version;
        out[5] = if self.progressive_frame { 0x01 } else { 0x02 } | (self.vbr as u8) << 4;
        out[6] = 0x80 | (self.crc_flag as u8) << 4;
        out[7] = 0xA0;
        be16(out, 0x18, self.height);
        be16(out, 0x1A, self.width);
        out[0x1C] = (((self.pixel_aspect.0 >> 8) & 3) << 2 | ((self.pixel_aspect.1 >> 8) & 3)) as u8;
        be16(out, 0x1D, self.height);
        out[0x1F] = self.pixel_aspect.0 as u8;
        out[0x20] = self.pixel_aspect.1 as u8;
        out[0x21] = self.bit_depth_code << 5 | 0x18;
        out[0x22] = 0x88 | (self.interlaced_source as u8) << 2;
        out[0x28..0x2C].copy_from_slice(&self.compression_id.to_be_bytes());
        out[0x2C] = (self.frame_encoding as u8) << 7 | self.sub_sampling << 5 | self.colour_volume << 1 | self.rgb as u8;
        out[0x30] = (self.timecode_present as u8) << 7;
        out[0x5F] = 0x01;
        out[0x167] = 0x02;
        let table_bytes = 4 * self.scan_lines.max(68);
        be16(out, 0x16A, table_bytes + 4);
        be16(out, 0x16C, self.scan_lines);
        out[0x16F] = 0x10;
        for (i, o) in self.scan_offsets.iter().enumerate() {
            out[0x170 + 4 * i..0x174 + 4 * i].copy_from_slice(&o.to_be_bytes());
        }
    }

    pub(crate) fn parse(data: &[u8]) -> Result<Header> {
        if data.len() < 0x280 {
            return Err(Error::Truncated);
        }
        let be16 = |at: usize| u16::from_be_bytes([data[at], data[at + 1]]) as u32;
        let hs = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let version = data[4];
        if version != 3 {
            return Err(Error::Unsupported("header version other than 3"));
        }
        if hs < 0x280 || hs as usize > data.len() {
            return Err(Error::Invalid("header size"));
        }
        let height = be16(0x18);
        let width = be16(0x1A);
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(Error::Invalid("raster size"));
        }
        if hs as usize != header_size(height) {
            return Err(Error::Invalid("header size does not match the raster height"));
        }
        let scan_lines = be16(0x16C);
        if scan_lines != height.div_ceil(16) {
            return Err(Error::Invalid("scan line count"));
        }
        let mut scan_offsets = Vec::with_capacity(scan_lines as usize);
        for i in 0..scan_lines as usize {
            let at = 0x170 + 4 * i;
            if at + 4 > hs as usize {
                return Err(Error::Invalid("scan index table overruns the header"));
            }
            scan_offsets.push(u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]));
        }
        Ok(Header {
            header_size: hs,
            version,
            vbr: data[5] & 0x10 != 0,
            progressive_frame: data[5] & 3 == 1,
            crc_flag: data[6] & 0x10 != 0,
            width,
            height,
            pixel_aspect: (((data[0x1C] >> 2 & 3) as u32) << 8 | data[0x1F] as u32, ((data[0x1C] & 3) as u32) << 8 | data[0x20] as u32),
            bit_depth_code: data[0x21] >> 5,
            interlaced_source: data[0x22] & 4 != 0,
            compression_id: u32::from_be_bytes([data[0x28], data[0x29], data[0x2A], data[0x2B]]),
            frame_encoding: data[0x2C] & 0x80 != 0,
            sub_sampling: data[0x2C] >> 5 & 3,
            colour_volume: data[0x2C] >> 1 & 3,
            rgb: data[0x2C] & 1 != 0,
            timecode_present: data[0x30] & 0x80 != 0,
            scan_lines,
            scan_offsets,
        })
    }
}
