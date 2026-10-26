// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! NAL unit framing: Annex-B byte streams, length-prefixed (MP4) samples,
//! emulation prevention removal (7.4.1) and the `avcC` decoder configuration record.

use crate::error::{Error, Result};

pub const NAL_SLICE: u8 = 1;
pub const NAL_DPA: u8 = 2;
pub const NAL_DPB: u8 = 3;
pub const NAL_DPC: u8 = 4;
pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;
pub const NAL_AUD: u8 = 9;
pub const NAL_END_SEQ: u8 = 10;
pub const NAL_END_STREAM: u8 = 11;

/// A NAL unit with its header decoded and its payload converted to RBSP.
#[derive(Debug, Clone)]
pub struct Nal {
    pub nal_ref_idc: u8,
    pub nal_unit_type: u8,
    pub rbsp: Vec<u8>,
}

impl Nal {
    /// Parse one NAL unit (header byte + payload with emulation prevention bytes).
    pub fn parse(data: &[u8]) -> Result<Nal> {
        let (&h, rest) = data.split_first().ok_or(Error::Bitstream("empty NAL unit"))?;
        if h & 0x80 != 0 {
            return Err(Error::Bitstream("forbidden_zero_bit set"));
        }
        let nal_unit_type = h & 0x1f;
        let mut payload = rest;
        // Prefix NAL / SVC / MVC extension headers carry 3 more header bytes; we skip those NALs
        // anyway, but keep the payload sane.
        if matches!(nal_unit_type, 14 | 20 | 21) && payload.len() >= 3 {
            payload = &payload[3..];
        }
        Ok(Nal { nal_ref_idc: (h >> 5) & 3, nal_unit_type, rbsp: unescape(payload) })
    }
}

/// Remove emulation prevention bytes (0x000003 -> 0x0000).
pub fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0usize;
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if zeros >= 2 && b == 3 {
            // Skip the emulation prevention byte.
            zeros = 0;
            i += 1;
            continue;
        }
        if b == 0 { zeros += 1 } else { zeros = 0 }
        out.push(b);
        i += 1;
    }
    out
}

/// Split an Annex-B byte stream into NAL unit payloads (start codes removed,
/// trailing zero bytes stripped).
pub fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let n = data.len();
    let mut i = 0;
    // Find first start code.
    let mut start = None;
    while i + 2 < n {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            start = Some(i + 3);
            i += 3;
            break;
        }
        i += 1;
    }
    let Some(mut s) = start else {
        return out;
    };
    while i + 2 < n {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let mut e = i;
            while e > s && data[e - 1] == 0 {
                e -= 1;
            }
            if e > s {
                out.push(&data[s..e]);
            }
            s = i + 3;
            i += 3;
            continue;
        }
        i += 1;
    }
    let mut e = n;
    while e > s && data[e - 1] == 0 {
        e -= 1;
    }
    if e > s {
        out.push(&data[s..e]);
    }
    out
}

/// True if the buffer starts with an Annex-B start code.
pub fn looks_like_annex_b(data: &[u8]) -> bool {
    data.len() >= 3 && data[0] == 0 && data[1] == 0 && (data[2] == 1 || (data.len() >= 4 && data[2] == 0 && data[3] == 1))
}

/// Split a length-prefixed sample (MP4 / avcC framing).
pub fn split_length_prefixed(data: &[u8], length_size: usize) -> Result<Vec<&[u8]>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if i + length_size > data.len() {
            return Err(Error::Bitstream("truncated NAL length prefix"));
        }
        let mut len = 0usize;
        for k in 0..length_size {
            len = (len << 8) | data[i + k] as usize;
        }
        i += length_size;
        if len > data.len() - i {
            return Err(Error::Bitstream("NAL length exceeds sample size"));
        }
        if len > 0 {
            out.push(&data[i..i + len]);
        }
        i += len;
    }
    Ok(out)
}

/// The `avcC` AVCDecoderConfigurationRecord (ISO/IEC 14496-15 5.3.3.1).
#[derive(Debug, Clone, Default)]
pub struct AvcConfig {
    pub profile_indication: u8,
    pub profile_compatibility: u8,
    pub level_indication: u8,
    /// NAL unit length field size in bytes (1, 2 or 4).
    pub nal_length_size: usize,
    /// SPS NAL units (with header byte, still escaped).
    pub sps: Vec<Vec<u8>>,
    /// PPS NAL units (with header byte, still escaped).
    pub pps: Vec<Vec<u8>>,
}

impl AvcConfig {
    /// Parse the payload of an `avcC` box.
    pub fn parse(data: &[u8]) -> Result<AvcConfig> {
        let bad = Error::InvalidConfig("truncated avcC");
        if data.len() < 7 {
            return Err(bad);
        }
        if data[0] != 1 {
            return Err(Error::InvalidConfig("unknown avcC configurationVersion"));
        }
        let mut cfg = AvcConfig {
            profile_indication: data[1],
            profile_compatibility: data[2],
            level_indication: data[3],
            nal_length_size: (data[4] & 3) as usize + 1,
            ..Default::default()
        };
        if cfg.nal_length_size == 3 {
            return Err(Error::InvalidConfig("invalid NAL length size 3"));
        }
        let mut i = 5;
        let num_sps = (data[i] & 0x1f) as usize;
        i += 1;
        for _ in 0..num_sps {
            if i + 2 > data.len() {
                return Err(bad);
            }
            let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
            i += 2;
            if i + len > data.len() {
                return Err(bad);
            }
            cfg.sps.push(data[i..i + len].to_vec());
            i += len;
        }
        if i >= data.len() {
            return Err(bad);
        }
        let num_pps = data[i] as usize;
        i += 1;
        for _ in 0..num_pps {
            if i + 2 > data.len() {
                return Err(bad);
            }
            let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
            i += 2;
            if i + len > data.len() {
                return Err(bad);
            }
            cfg.pps.push(data[i..i + len].to_vec());
            i += len;
        }
        // Any High-profile extension fields (chroma_format etc.) are redundant with the SPS.
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_removes_epb() {
        assert_eq!(unescape(&[0, 0, 3, 1, 0, 0, 3, 0, 0, 3]), vec![0, 0, 1, 0, 0, 0, 0]);
        assert_eq!(unescape(&[1, 0, 3, 0]), vec![1, 0, 3, 0]);
    }

    #[test]
    fn annex_b_split() {
        let s = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 0, 1, 0x65, 9];
        let v = split_annex_b(&s);
        assert_eq!(v, vec![&[0x67u8, 1, 2][..], &[0x68, 3][..], &[0x65, 9][..]]);
    }

    #[test]
    fn length_prefixed_split() {
        let s = [0, 0, 0, 2, 0x65, 1, 0, 0, 0, 1, 0x41];
        let v = split_length_prefixed(&s, 4).unwrap();
        assert_eq!(v, vec![&[0x65u8, 1][..], &[0x41][..]]);
        assert!(split_length_prefixed(&s[..5], 4).is_err());
    }
}
