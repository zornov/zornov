// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Length-prefixed NAL unit handling for AVC / HEVC samples.
//!
//! MP4 stores each NAL unit behind a big-endian length of `length_size` bytes
//! (from `avcC` / `hvcC`). These helpers split samples into NAL units and
//! convert them to Annex-B (`00 00 00 01` start codes).

use crate::error::{Error, Result};

/// Iterator over the NAL units of one sample. Yields an error once on a bad
/// length prefix and then stops.
pub struct NalIter<'a> {
    data: &'a [u8],
    length_size: usize,
    failed: bool,
}

impl<'a> NalIter<'a> {
    /// `length_size` must be 1..=4.
    pub fn new(data: &'a [u8], length_size: u8) -> Result<Self> {
        if !(1..=4).contains(&length_size) {
            return Err(Error::malformed(format!("NAL length size {length_size}")));
        }
        Ok(NalIter { data, length_size: length_size as usize, failed: false })
    }
}

impl<'a> Iterator for NalIter<'a> {
    type Item = Result<&'a [u8]>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.data.is_empty() {
            return None;
        }
        if self.data.len() < self.length_size {
            self.failed = true;
            return Some(Err(Error::Truncated("NAL length prefix")));
        }
        let (prefix, rest) = self.data.split_at(self.length_size);
        let len = prefix.iter().fold(0usize, |acc, &b| (acc << 8) | b as usize);
        if len > rest.len() {
            self.failed = true;
            return Some(Err(Error::Truncated("NAL unit")));
        }
        let (nal, tail) = rest.split_at(len);
        self.data = tail;
        Some(Ok(nal))
    }
}

/// Splits a length-prefixed sample into its NAL units (without prefixes).
pub fn split_nals(data: &[u8], length_size: u8) -> Result<Vec<&[u8]>> {
    NalIter::new(data, length_size)?.collect()
}

/// Converts a length-prefixed sample into an Annex-B byte stream.
pub fn to_annex_b(data: &[u8], length_size: u8) -> Result<Vec<u8>> {
    Ok(nals_to_annex_b(split_nals(data, length_size)?))
}

/// Joins NAL units into an Annex-B byte stream with 4-byte start codes.
pub fn nals_to_annex_b<'a>(nals: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_convert() {
        let data = [0, 0, 0, 2, 0x65, 1, 0, 0, 0, 1, 0x06];
        assert_eq!(split_nals(&data, 4).unwrap(), vec![&[0x65, 1][..], &[0x06][..]]);
        assert_eq!(to_annex_b(&data, 4).unwrap(), [0, 0, 0, 1, 0x65, 1, 0, 0, 0, 1, 0x06]);
        assert_eq!(split_nals(&[1, 9, 2, 7, 7], 1).unwrap(), vec![&[9][..], &[7, 7][..]]);
        assert!(split_nals(&[0, 5, 1], 2).is_err());
        assert!(split_nals(&[0, 0], 4).is_err());
        assert!(split_nals(&data, 0).is_err());
        assert!(split_nals(&[], 4).unwrap().is_empty());
    }
}
