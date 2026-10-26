// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Video decoder configuration records: `avcC` (ISO/IEC 14496-15 §5.3.3) and
//! `hvcC` (§8.3.3).

use crate::r#box::Reader;
use crate::error::{Error, Result};
use crate::nal::nals_to_annex_b;

/// Parsed `AVCDecoderConfigurationRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvcConfig {
    /// 66 baseline, 77 main, 100 high, ...
    pub profile_idc: u8,
    pub profile_compatibility: u8,
    pub level_idc: u8,
    /// Bytes in each NAL length prefix (1, 2 or 4).
    pub nal_length_size: u8,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
    /// High-profile extension fields, when present.
    pub chroma_format: Option<u8>,
    pub bit_depth_luma: Option<u8>,
    pub bit_depth_chroma: Option<u8>,
    pub sps_ext: Vec<Vec<u8>>,
    /// The whole record as stored in the file.
    pub raw: Vec<u8>,
}

impl AvcConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "avcC");
        if r.u8()? != 1 {
            return Err(Error::malformed("avcC configurationVersion != 1"));
        }
        let profile_idc = r.u8()?;
        let profile_compatibility = r.u8()?;
        let level_idc = r.u8()?;
        let nal_length_size = (r.u8()? & 3) + 1;
        if nal_length_size == 3 {
            return Err(Error::malformed("avcC NAL length size 3"));
        }
        let n_sps = r.u8()? & 0x1f;
        let sps = read_u16_nals(&mut r, n_sps as usize)?;
        let n_pps = r.u8()?;
        let pps = read_u16_nals(&mut r, n_pps as usize)?;

        let mut cfg = AvcConfig {
            profile_idc,
            profile_compatibility,
            level_idc,
            nal_length_size,
            sps,
            pps,
            chroma_format: None,
            bit_depth_luma: None,
            bit_depth_chroma: None,
            sps_ext: Vec::new(),
            raw: data.to_vec(),
        };
        // The extension is optional and some writers emit it for any profile,
        // or truncate it; a broken extension does not invalidate the record.
        if matches!(profile_idc, 100 | 110 | 122 | 144) && r.remaining() >= 4 {
            let mut ext = r.clone();
            let parsed = (|| -> Result<_> {
                let chroma = ext.u8()? & 3;
                let luma = (ext.u8()? & 7) + 8;
                let chromab = (ext.u8()? & 7) + 8;
                let n = ext.u8()? as usize;
                Ok((chroma, luma, chromab, read_u16_nals(&mut ext, n)?))
            })();
            if let Ok((c, l, cb, e)) = parsed {
                cfg.chroma_format = Some(c);
                cfg.bit_depth_luma = Some(l);
                cfg.bit_depth_chroma = Some(cb);
                cfg.sps_ext = e;
            }
        }
        Ok(cfg)
    }

    /// SPS and PPS as an Annex-B byte stream (start code before each NAL).
    pub fn annex_b_parameter_sets(&self) -> Vec<u8> {
        nals_to_annex_b(self.sps.iter().chain(&self.sps_ext).chain(&self.pps).map(|v| v.as_slice()))
    }
}

/// One NAL unit array from `hvcC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HevcNalArray {
    pub completeness: bool,
    /// HEVC `nal_unit_type` (32 VPS, 33 SPS, 34 PPS, 39/40 SEI).
    pub nal_type: u8,
    pub nalus: Vec<Vec<u8>>,
}

/// Parsed `HEVCDecoderConfigurationRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HevcConfig {
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    /// 1 Main, 2 Main 10, 3 Main Still Picture, 4 range extensions, ...
    pub general_profile_idc: u8,
    pub general_profile_compatibility_flags: u32,
    pub general_constraint_indicator_flags: u64,
    pub general_level_idc: u8,
    pub min_spatial_segmentation_idc: u16,
    pub parallelism_type: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub avg_frame_rate: u16,
    pub constant_frame_rate: u8,
    pub num_temporal_layers: u8,
    pub temporal_id_nested: bool,
    /// Bytes in each NAL length prefix (1, 2 or 4).
    pub nal_length_size: u8,
    pub arrays: Vec<HevcNalArray>,
    /// The whole record as stored in the file.
    pub raw: Vec<u8>,
}

impl HevcConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data, "hvcC");
        if r.u8()? != 1 {
            return Err(Error::malformed("hvcC configurationVersion != 1"));
        }
        let b = r.u8()?;
        let general_profile_compatibility_flags = r.u32()?;
        let hi = r.u16()? as u64;
        let general_constraint_indicator_flags = (hi << 32) | r.u32()? as u64;
        let general_level_idc = r.u8()?;
        let min_spatial_segmentation_idc = r.u16()? & 0x0fff;
        let parallelism_type = r.u8()? & 3;
        let chroma_format_idc = r.u8()? & 3;
        let bit_depth_luma = (r.u8()? & 7) + 8;
        let bit_depth_chroma = (r.u8()? & 7) + 8;
        let avg_frame_rate = r.u16()?;
        let t = r.u8()?;
        let nal_length_size = (t & 3) + 1;
        if nal_length_size == 3 {
            return Err(Error::malformed("hvcC NAL length size 3"));
        }
        let n_arrays = r.u8()?;
        let mut arrays = Vec::with_capacity(n_arrays as usize);
        for _ in 0..n_arrays {
            let h = r.u8()?;
            let n = r.u16()? as usize;
            arrays.push(HevcNalArray {
                completeness: h & 0x80 != 0,
                nal_type: h & 0x3f,
                nalus: read_u16_nals(&mut r, n)?,
            });
        }
        Ok(HevcConfig {
            general_profile_space: b >> 6,
            general_tier_flag: b & 0x20 != 0,
            general_profile_idc: b & 0x1f,
            general_profile_compatibility_flags,
            general_constraint_indicator_flags,
            general_level_idc,
            min_spatial_segmentation_idc,
            parallelism_type,
            chroma_format_idc,
            bit_depth_luma,
            bit_depth_chroma,
            avg_frame_rate,
            constant_frame_rate: t >> 6,
            num_temporal_layers: (t >> 3) & 7,
            temporal_id_nested: t & 4 != 0,
            nal_length_size,
            arrays,
            raw: data.to_vec(),
        })
    }

    /// All NAL units of the given `nal_unit_type`.
    pub fn nalus_of_type(&self, nal_type: u8) -> Vec<&[u8]> {
        self.arrays
            .iter()
            .filter(|a| a.nal_type == nal_type)
            .flat_map(|a| a.nalus.iter().map(|n| n.as_slice()))
            .collect()
    }

    pub fn vps(&self) -> Vec<&[u8]> {
        self.nalus_of_type(32)
    }

    pub fn sps(&self) -> Vec<&[u8]> {
        self.nalus_of_type(33)
    }

    pub fn pps(&self) -> Vec<&[u8]> {
        self.nalus_of_type(34)
    }

    /// Prefix and suffix SEI units.
    pub fn sei(&self) -> Vec<&[u8]> {
        let mut v = self.nalus_of_type(39);
        v.extend(self.nalus_of_type(40));
        v
    }

    /// VPS, SPS, PPS then SEI as an Annex-B byte stream.
    pub fn annex_b_parameter_sets(&self) -> Vec<u8> {
        let mut nals = self.vps();
        nals.extend(self.sps());
        nals.extend(self.pps());
        nals.extend(self.sei());
        nals_to_annex_b(nals)
    }
}

fn read_u16_nals(r: &mut Reader<'_>, n: usize) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for _ in 0..n {
        let len = r.u16()? as usize;
        out.push(r.bytes(len)?.to_vec());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_roundtrip() {
        let rec = [1, 100, 0, 31, 0xff, 0xe1, 0, 3, 0x67, 1, 2, 1, 0, 2, 0x68, 9, 0xfd, 0xf8, 0xf8, 0];
        let c = AvcConfig::parse(&rec).unwrap();
        assert_eq!((c.profile_idc, c.level_idc, c.nal_length_size), (100, 31, 4));
        assert_eq!(c.sps, vec![vec![0x67, 1, 2]]);
        assert_eq!(c.pps, vec![vec![0x68, 9]]);
        assert_eq!(c.chroma_format, Some(1));
        assert_eq!(c.annex_b_parameter_sets(), [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 9]);
        assert!(AvcConfig::parse(&rec[..9]).is_err());
    }
}
