// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! NAL unit header parsing and access unit splitting.

use crate::error::{Error, Result};

pub const TRAIL_N: u8 = 0;
pub const TRAIL_R: u8 = 1;
pub const TSA_N: u8 = 2;
pub const RADL_N: u8 = 6;
pub const RADL_R: u8 = 7;
pub const RASL_N: u8 = 8;
pub const RASL_R: u8 = 9;
pub const RSV_VCL_N14: u8 = 14;
pub const BLA_W_LP: u8 = 16;
pub const BLA_W_RADL: u8 = 17;
pub const BLA_N_LP: u8 = 18;
pub const IDR_W_RADL: u8 = 19;
pub const IDR_N_LP: u8 = 20;
pub const CRA_NUT: u8 = 21;
pub const RSV_IRAP_23: u8 = 23;
pub const VPS_NUT: u8 = 32;
pub const SPS_NUT: u8 = 33;
pub const PPS_NUT: u8 = 34;
pub const AUD_NUT: u8 = 35;
pub const EOS_NUT: u8 = 36;
pub const EOB_NUT: u8 = 37;
pub const FD_NUT: u8 = 38;
pub const PREFIX_SEI: u8 = 39;
pub const SUFFIX_SEI: u8 = 40;

/// Two-byte NAL unit header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalHeader {
    pub nal_type: u8,
    pub layer_id: u8,
    pub temporal_id: u8,
}

impl NalHeader {
    pub fn parse(data: &[u8]) -> Result<NalHeader> {
        if data.len() < 2 {
            return Err(Error::UnexpectedEof);
        }
        if data[0] & 0x80 != 0 {
            return Err(Error::Invalid("forbidden_zero_bit set"));
        }
        let nal_type = (data[0] >> 1) & 0x3f;
        let layer_id = ((data[0] & 1) << 5) | (data[1] >> 3);
        let tid_plus1 = data[1] & 7;
        if tid_plus1 == 0 {
            return Err(Error::Invalid("nuh_temporal_id_plus1 is zero"));
        }
        Ok(NalHeader { nal_type, layer_id, temporal_id: tid_plus1 - 1 })
    }

    pub fn is_vcl(&self) -> bool {
        self.nal_type < 32
    }
    pub fn is_irap(&self) -> bool {
        (BLA_W_LP..=RSV_IRAP_23).contains(&self.nal_type)
    }
    pub fn is_idr(&self) -> bool {
        self.nal_type == IDR_W_RADL || self.nal_type == IDR_N_LP
    }
    pub fn is_bla(&self) -> bool {
        (BLA_W_LP..=BLA_N_LP).contains(&self.nal_type)
    }
    pub fn is_cra(&self) -> bool {
        self.nal_type == CRA_NUT
    }
    pub fn is_rasl(&self) -> bool {
        self.nal_type == RASL_N || self.nal_type == RASL_R
    }
    pub fn is_radl(&self) -> bool {
        self.nal_type == RADL_N || self.nal_type == RADL_R
    }
    /// Sub-layer non-reference picture (TRAIL_N, TSA_N, ..., RSV_VCL_N14).
    pub fn is_sub_layer_non_ref(&self) -> bool {
        self.nal_type <= RSV_VCL_N14 && self.nal_type % 2 == 0
    }
}

/// Splits a length-prefixed (ISO/IEC 14496-15) access unit into NAL units.
pub fn split_length_prefixed(data: &[u8], nal_length_size: usize) -> Result<Vec<&[u8]>> {
    if !(1..=4).contains(&nal_length_size) {
        return Err(Error::Invalid("nal_length_size must be 1..=4"));
    }
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        if pos + nal_length_size > data.len() {
            return Err(Error::UnexpectedEof);
        }
        let mut len = 0usize;
        for i in 0..nal_length_size {
            len = (len << 8) | data[pos + i] as usize;
        }
        pos += nal_length_size;
        if len > data.len() - pos {
            return Err(Error::UnexpectedEof);
        }
        if len > 0 {
            out.push(&data[pos..pos + len]);
        }
        pos += len;
    }
    Ok(out)
}

/// Splits an Annex B byte stream (start-code delimited) into NAL units.
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let n = data.len();
    let mut i = 0usize;
    // find first start code
    let mut start: Option<usize> = None;
    while i + 2 < n {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                let mut e = i;
                while e > s && data[e - 1] == 0 {
                    e -= 1;
                }
                if e > s {
                    out.push(&data[s..e]);
                }
            }
            i += 3;
            start = Some(i);
            continue;
        }
        i += 1;
    }
    if let Some(s) = start {
        if s < n {
            out.push(&data[s..n]);
        }
    }
    out
}
